#!/usr/bin/env bash
# Axiom gate for the Botho formal model (#1705, hardened in #1770).
#
# Every theorem listed in Theorems.lean must depend only on axioms named in
# axioms-allowlist.txt (never `sorryAx`). The gate checks this in four layers,
# so that no single trick in the model can fake a pass:
#
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
# Usage: check-axioms.sh [FORMAL_ROOT]
#   FORMAL_ROOT defaults to the directory above this script.
# Environment: LAKE overrides the lake binary (default: lake).
set -euo pipefail

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

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
out="$tmp/out"

lakefile_preflight

echo "== axiom gate: manifest syntax"
if ! python3 - "$manifest" "$gate_lib" >"$tmp/commands" <<'PY'
import re
import sys

path, lib = sys.argv[1:]
with open(path, encoding="utf-8") as fh:
    src = fh.read()


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

echo "== axiom gate: lake env lean --json Theorems.lean"
if ! (cd "$root" && "$lake" env lean --json Theorems.lean) >"$tmp/json" 2>&1; then
  cat "$tmp/json"
  echo "FAIL: Theorems.lean does not compile" >&2
  exit 1
fi

echo "== axiom gate: independent axiom walk (AxiomReport.lean)"
names=()
while IFS=$'\t' read -r _ _ name; do names+=("$name"); done <"$tmp/commands"
run_report "$tmp/report" "${names[@]}"

python3 - "$tmp/commands" "$tmp/json" "$tmp/report" "$allowlist" <<'PY'
import json
import re
import sys

commands_path, json_path, report_path, allowlist = sys.argv[1:]

commands = {}  # (line, col) -> name
order = []
with open(commands_path, encoding="utf-8") as fh:
    for row in fh.read().splitlines():
        line, col, name = row.split("\t")
        commands[(int(line), int(col))] = name
        order.append(name)

allowed = set()
with open(allowlist, encoding="utf-8") as fh:
    for row in fh:
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
with open(json_path, encoding="utf-8") as fh:
    for raw in fh.read().splitlines():
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
with open(report_path, encoding="utf-8") as fh:
    for row in fh.read().splitlines():
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
