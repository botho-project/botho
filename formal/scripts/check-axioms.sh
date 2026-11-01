#!/usr/bin/env bash
# Axiom gate for the Botho formal model (#1705, hardened in #1770).
#
# Every theorem listed in Theorems.lean must depend only on axioms named in
# axioms-allowlist.txt (never `sorryAx`). The gate checks this in four layers,
# so that no single trick in the model can fake a pass, after two checks that
# run before any `lake` call:
#
#   0. Pinned layout and source scan. lakefile.toml, lean-toolchain and
#      lake-manifest.json must match the pins in gate-common.sh, and the
#      source scan (gate_scan.py, shared with the sorry gate) must pass. The
#      scan's import allow-list keeps `import Lean` and friends out of the
#      model, so no simproc, elaborator or tactic written in the model can run
#      during `lake build` and rewrite the compiled library that layers 2 and
#      4 read. The layers below cannot detect such a rewrite on their own.
#   1. Strict manifest. After comments are removed, Theorems.lean may contain
#      only `import` lines (of Botho modules) followed by `#print axioms NAME`
#      lines, one per line, with no duplicate names. Nothing else (no `#eval`, no declarations) can
#      appear in the file whose output is parsed.
#   2. Kernel replay. `leanchecker` re-checks every declaration of the Botho
#      library in the kernel, so a declaration admitted with kernel checking
#      switched off (`debug.skipKernelTC`, `addDeclWithoutChecking`) fails.
#   3. Positioned results. `lean --json Theorems.lean` must print only Lean
#      messages (JSON lines), with no errors or warnings. Exactly one
#      `#print axioms` result is accepted per manifest command, matched by the
#      command's source position and theorem name. Output printed anywhere
#      else (`#eval IO.println`, imported initializers) cannot stand in for a
#      result.
#   4. Independent walk. AxiomReport.lean imports the compiled library without
#      elaborating any of its syntax and walks the declaration bodies itself.
#      Its axiom sets must equal Lean's `#print axioms` results, so a model that
#      redefines `#print axioms` (macro_rules / elab_rules) is caught. The walk
#      is the authority for the allow-list check. It also fails if any
#      declaration in the library reaches `sorryAx`, listed or not.
#
# Gate inputs are read before the first `lake` call (#1770 review). Model code
# runs during `lake build` (for example an `@[init]` initializer), so it could
# otherwise rewrite axioms-allowlist.txt, Theorems.lean or AxiomReport.lean
# before the gate reads them. The allow-list, the manifest and the report
# program are held in shell variables from the start; the manifest reaches
# Lean on stdin, and the report program is written out just before it runs.
# The walk runs before Theorems.lean is compiled, because that compile runs
# the initializers of every imported module again. The whole script is one
# function, so bash parses it completely before running anything and a
# rewrite of this file during the build has no effect. These are defence in
# depth, not a sandbox: see "Limits" in formal/README.md.
#
# Usage: check-axioms.sh [--no-scan] [FORMAL_ROOT]
#   FORMAL_ROOT defaults to the directory above this script.
#   --no-scan  skip the source scan in step 0 (the layout pin still runs).
#              Only the self-test uses this, to prove each later layer works
#              on its own. CI never passes it.
# Environment: LAKE overrides the lake binary (default: lake).
set -euo pipefail

main() {
scan=1
while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-scan) scan=0; shift ;;
    *) break ;;
  esac
done
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="${1:-$(cd "$here/.." && pwd)}"
root="$(cd "$root" && pwd)"
lake="${LAKE:-lake}"
manifest="$root/Theorems.lean"
allowlist="$root/axioms-allowlist.txt"
# The library the gates check: `Botho` and every `Botho.*` module.
gate_lib=Botho
# shellcheck disable=SC1091  # gate-common.sh sits next to this script
. "$here/gate-common.sh"

for f in "$manifest" "$allowlist"; do
  if [[ ! -f "$f" ]]; then
    echo "FAIL: missing $f" >&2
    exit 1
  fi
done

# Snapshot the gate inputs before any `lake` call (see the header). From here
# on the gate reads only these variables, never the files.
manifest_src="$(cat "$manifest")"
allowlist_src="$(cat "$allowlist")"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
out="$tmp/out"

lakefile_preflight

if [[ "$scan" == 1 ]]; then
  echo "== axiom gate: source scan ($root)"
  if ! source_scan; then
    exit 1
  fi
fi

echo "== axiom gate: manifest syntax"
if ! GATE_MANIFEST="$manifest_src" python3 - "$manifest" "$gate_lib" \
  >"$tmp/commands" <<'PY'
import os
import re
import sys

path, lib = sys.argv[1:]
src = os.environ["GATE_MANIFEST"]


def blank_comments(s):
    """Replace comments with spaces, keeping line/column positions. Nested
    block comments are handled. The manifest may not contain string or char
    literals at all (rejected below), so quotes need no special handling."""
    out, i, n, depth = [], 0, len(s), 0
    while i < n:
        two = s[i:i + 2]
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
                out.append("\n" if s[i] == "\n" else " ")
                i += 1
        elif two == "/-":
            depth = 1
            out.append("  ")
            i += 2
        elif two == "--":
            while i < n and s[i] != "\n":
                out.append(" ")
                i += 1
        else:
            out.append(s[i])
            i += 1
    if depth:
        sys.exit(f"{path}: unterminated block comment")
    return "".join(out)


# A theorem name: no spaces, quotes (a prime is fine after the first char),
# brackets or guillemets.
NAME = r"[^\s\"'`«»(){}\[\],;:][^\s\"`«»(){}\[\],;:]*"
IMPORT = re.compile(r"^import\s+" + NAME + r"(?:\s+" + NAME + r")*\s*$")
PRINT = re.compile(r"^(#print[ \t]+axioms)[ \t]+(" + NAME + r")\s*$")
errors, seen, commands, imports_done = [], {}, [], False
for lineno, line in enumerate(blank_comments(src).split("\n"), 1):
    body = line.strip()
    if not body:
        continue
    col = len(line) - len(line.lstrip())
    m = PRINT.match(body)
    if m:
        imports_done = True
        name = m.group(2)
        while name.startswith("_root_."):
            name = name[len("_root_."):]
        if name.startswith(".") or name.endswith(".") or ".." in name:
            errors.append(f"line {lineno}: malformed theorem name {m.group(2)!r}")
        elif name in seen:
            errors.append(f"line {lineno}: {name} already listed on line {seen[name]}")
        else:
            seen[name] = lineno
            commands.append(f"{lineno}\t{col}\t{name}")
        continue
    if not imports_done and IMPORT.match(body):
        mods = body.split()[1:]
        others = [m for m in mods if m != lib and not m.startswith(lib + ".")]
        if others:
            errors.append(f"line {lineno}: imports outside the {lib} library: {others}")
        continue
    errors.append(
        f"line {lineno}: only `import` lines and `#print axioms NAME` lines are "
        f"allowed in the manifest, found: {body[:80]}"
    )
if not commands:
    errors.append("no '#print axioms' lines (empty manifest)")
if errors:
    for e in errors:
        print(f"FAIL: {path}: {e}", file=sys.stderr)
    sys.exit(1)
print("\n".join(commands))
PY
then
  echo "FAIL: Theorems.lean is not a plain manifest" >&2
  exit 1
fi
commands="$(cat "$tmp/commands")"

echo "== axiom gate: lake build ($root)"
if ! (cd "$root" && "$lake" build) >"$out" 2>&1; then
  cat "$out"
  echo "FAIL: lake build failed" >&2
  exit 1
fi

echo "== axiom gate: leanchecker $gate_lib (kernel replay)"
if ! (cd "$root" && "$lake" env leanchecker "$gate_lib") >"$out" 2>&1; then
  cat "$out"
  echo "FAIL: leanchecker rejected a declaration in $gate_lib" >&2
  exit 1
fi

# The walk runs before Theorems.lean is compiled: compiling Theorems.lean runs
# the initializers of every imported module, which could tamper with the
# compiled library the walk reads.
echo "== axiom gate: independent axiom walk (AxiomReport.lean)"
names=()
while IFS=$'\t' read -r _ _ name; do names+=("$name"); done <<<"$commands"
run_report "$tmp/report" "${names[@]}"
report="$(cat "$tmp/report")"

echo "== axiom gate: lake env lean --json Theorems.lean (snapshot, on stdin)"
if ! json="$(cd "$root" && "$lake" env lean --json --stdin <<<"$manifest_src" 2>&1)"; then
  printf '%s\n' "$json"
  echo "FAIL: Theorems.lean does not compile" >&2
  exit 1
fi

GATE_COMMANDS="$commands" GATE_JSON="$json" GATE_REPORT="$report" \
  GATE_ALLOWLIST="$allowlist_src" python3 - <<'PY'
import json
import os
import re
import sys

commands = {}  # (line, col) -> name
order = []
for row in os.environ["GATE_COMMANDS"].splitlines():
    line, col, name = row.split("\t")
    commands[(int(line), int(col))] = name
    order.append(name)

# The allow-list as read before `lake build` (not the file, which model code
# could have rewritten during the build).
allowed = set()
for row in os.environ["GATE_ALLOWLIST"].splitlines():
    name = row.split("#", 1)[0].strip()
    if name:
        allowed.add(name)

errors = []

# Layer 3: positioned `#print axioms` results from `lean --json`.
RESULT = re.compile(
    r"'(.+?)' (?:depends on axioms: \[(.*)\]|does not depend on any axioms)\s*",
    re.S,
)
printed = {}  # name -> sorted axiom list
for raw in os.environ["GATE_JSON"].splitlines():
    if not raw.strip():
        continue
    try:
        msg = json.loads(raw)
        pos = (int(msg["pos"]["line"]), int(msg["pos"]["column"]))
        severity, data = msg["severity"], msg["data"]
    except (ValueError, KeyError, TypeError):
        errors.append(f"lean printed a line that is not a Lean message: {raw[:120]}")
        continue
    if severity != "information":
        errors.append(f"Theorems.lean:{pos[0]}:{pos[1]}: {severity}: {data[:200]}")
        continue
    name = commands.get(pos)
    if name is None:
        errors.append(
            f"Theorems.lean:{pos[0]}:{pos[1]}: message not at a manifest "
            f"'#print axioms' command: {data[:120]!r}"
        )
        continue
    m = RESULT.fullmatch(data)
    if not m or m.group(1) != name:
        errors.append(
            f"Theorems.lean:{pos[0]}:{pos[1]}: not a '#print axioms {name}' "
            f"result: {data[:120]!r}"
        )
        continue
    if name in printed:
        errors.append(f"Theorems.lean:{pos[0]}:{pos[1]}: second result for {name}")
        continue
    listed = m.group(2)
    printed[name] = sorted(
        a.strip() for a in (listed or "").replace("\n", " ").split(",") if a.strip()
    )
for name in order:
    if name not in printed:
        errors.append(f"no '#print axioms' result for {name}")

# Layer 4: the independent walk.
walked, swept = {}, None
for row in os.environ["GATE_REPORT"].splitlines():
    parts = row.split("\t")
    if parts[0] == "AXIOMS":
        walked[parts[1]] = sorted(a for a in parts[2].split(",") if a)
    elif parts[0] == "MISSING":
        errors.append(f"{parts[1]} is not a declaration of the compiled library")
    elif parts[0] == "SORRY":
        errors.append(f"{parts[1]} reaches sorryAx (a sorry is reachable)")
    elif parts[0] == "DONE":
        swept = int(parts[1])
for name in order:
    if name not in walked:
        continue
    if name in printed and printed[name] != walked[name]:
        errors.append(
            f"{name}: '#print axioms' says {printed[name]} but the independent "
            f"walk finds {walked[name]} (is '#print axioms' overridden?)"
        )
    for ax in walked[name]:
        if ax == "sorryAx":
            errors.append(f"{name} depends on sorryAx (a sorry is reachable)")
        elif ax not in allowed:
            errors.append(f"{name} depends on non-allow-listed axiom {ax}")

if errors:
    for e in errors:
        print(f"FAIL: {e}", file=sys.stderr)
    sys.exit(1)
for name in order:
    axs = ", ".join(walked[name]) or "no axioms"
    print(f"  {name}: {axs}")
print(
    f"OK: axiom gate passed ({len(order)} theorems; {swept} library declarations "
    f"swept; allow-list: {', '.join(sorted(allowed))})"
)
PY
}

main "$@"
