#!/usr/bin/env bash
# Trusted verifier for the Botho formal model (#1799).
#
# The `verify` job in .github/workflows/formal.yml runs this script. It checks
# out the script, AxiomReport.lean, check-sorry.sh, axioms-allowlist.txt and
# lakefile.toml from the BASE branch, so a pull request cannot change what this script does
# or which axioms it accepts. It checks compiled .olean files that the
# untrusted `build` job uploaded. It never runs `lake`, never compiles a model
# source, and never runs model code:
#
#   - `leanchecker` and AxiomReport.lean read the .olean files with
#     `importModules (loadExts := false)`. `[init]` declarations only run
#     when extensions are loaded (`loadExts := true`), so a model initializer
#     does not run here. The self-test (selftest-verify.sh) checks this with
#     the #1783 review PoCs.
#   - No `.lean` file of the model is passed to Lean. Elaborating even
#     `import Botho` would run the model's initializers.
#   - Lean finds its own library (Init) through LEAN_SYSROOT, which this
#     script sets to --sysroot for every Lean process. Without it,
#     `leanchecker` and AxiomReport.lean ask the `lean` on PATH for its
#     prefix, so --sysroot would not decide which Init is trusted, and the
#     verify job, which has no `lean` on PATH, could not run at all.
#
# Usage: verify-artifacts.sh --lib DIR --sources DIR [--sysroot DIR]
#                            [--allowlist FILE] [--base-lakefile FILE]
#                            [--no-scan]
#   --lib DIR        the downloaded build artifact: Botho.olean and
#                    Botho/**/*.olean (the `.lake/build/lib/lean` directory).
#   --sources DIR    the pull request's formal/ tree. Read as data only: the
#                    manifest (Theorems.lean) is parsed as text, and the base
#                    branch's check-sorry.sh scans the sources.
#   --sysroot DIR    the Lean toolchain (default: `lean --print-prefix`).
#   --allowlist FILE default: ../axioms-allowlist.txt next to this script,
#                    which in CI is the base branch's copy.
#   --base-lakefile FILE
#                    the base branch's lakefile.toml (default:
#                    ../lakefile.toml next to this script).
#   --no-scan        skip check 2. Only selftest-verify.sh uses this, to show
#                    that checks 2-4 reject each fixture on their own. CI
#                    never passes it.
#
# Checks, in order (any failure exits 1):
#   1. Source layout: the pull request's lakefile.toml must compile the same
#      sources as the base branch's. Both are parsed with tomllib; the
#      package-level `srcDir` and, for every [[lean_lib]] and [[lean_exe]],
#      `name`, `srcDir`, `roots`, `globs` and `root` must be identical, so
#      the set of libraries and executables is too. Other keys (version,
#      options, [[require]] pins) may change. A lakefile.lean next to either
#      lakefile.toml, or a lakefile.toml tomllib cannot parse, is rejected.
#      Without this check a pull request could keep the scanned Botho/ as a
#      decoy and compile another copy (`srcDir = "src"`) whose theorems
#      state something else.
#   2. Source scan: `check-sorry.sh --no-build` from this directory (the base
#      branch's copy, so whatever scan the base branch has) over --sources.
#   3. Artifact intake: --lib holds only regular files named Botho.<ext> or
#      Botho/**/*.<ext> with <ext> one of olean, olean.server, olean.private,
#      ir. They are copied to a private directory, which is the only entry on
#      LEAN_PATH, so the artifact cannot shadow a toolchain module (Init,
#      Lean, ...).
#   4. Kernel replay: `leanchecker Botho`.
#   5. Independent walk: AxiomReport.lean over the copied library. Fails if
#      - a theorem listed in Theorems.lean is missing or depends on an axiom
#        outside the allow-list (or on sorryAx);
#      - any library declaration reaches sorryAx;
#      - a library module declares an axiom outside the allow-list, imports a
#        module outside Init / Botho, or declares an `[init]` initializer;
#      - the compiled module set differs from the source module set
#        (Botho.lean + Botho/**/*.lean) or from the files in the artifact.
#
# What this does not check: that the .olean files are what the sources
# compile to. See "Limits" in formal/README.md.
set -euo pipefail

main() {
local here lib="" sources="" sysroot="" allowlist="" base_lakefile="" scan=1
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --lib) lib="$2"; shift 2 ;;
    --sources) sources="$2"; shift 2 ;;
    --sysroot) sysroot="$2"; shift 2 ;;
    --allowlist) allowlist="$2"; shift 2 ;;
    --base-lakefile) base_lakefile="$2"; shift 2 ;;
    --no-scan) scan=0; shift ;;
    *) echo "usage: $0 --lib DIR --sources DIR [--sysroot DIR] [--allowlist FILE] [--base-lakefile FILE] [--no-scan]" >&2
       exit 2 ;;
  esac
done
if [[ -z "$lib" || -z "$sources" ]]; then
  echo "usage: $0 --lib DIR --sources DIR [--sysroot DIR] [--allowlist FILE] [--base-lakefile FILE] [--no-scan]" >&2
  exit 2
fi
lib="$(cd "$lib" && pwd)"
sources="$(cd "$sources" && pwd)"
allowlist="${allowlist:-$here/../axioms-allowlist.txt}"
base_lakefile="${base_lakefile:-$here/../lakefile.toml}"
if [[ -z "$sysroot" ]]; then
  sysroot="$(lean --print-prefix)"
fi
sysroot="$(cd "$sysroot" && pwd)"
local lean="$sysroot/bin/lean" checker="$sysroot/bin/leanchecker"
local f
for f in "$lean" "$checker"; do
  if [[ ! -x "$f" ]]; then
    echo "FAIL: missing $f" >&2
    exit 1
  fi
done
for f in "$allowlist" "$base_lakefile" "$here/AxiomReport.lean" \
  "$here/check-sorry.sh" "$sources/Theorems.lean"; do
  if [[ ! -f "$f" ]]; then
    echo "FAIL: missing $f" >&2
    exit 1
  fi
done
local gate_lib=Botho

local tmp
tmp="$(mktemp -d)"
# shellcheck disable=SC2064  # expand now: tmp is local
trap "rm -rf '$tmp'" EXIT
mkdir "$tmp/lib" "$tmp/cwd"

echo "== verify: source layout (lakefile.toml against $base_lakefile)"
if ! python3 -I - "$base_lakefile" "$sources" <<'PY'
import json
import os
import sys

try:
    import tomllib
except ImportError:
    sys.exit("FAIL: python3 has no tomllib (needs Python 3.11 or later)")

base_file, sources = sys.argv[1:]
base_dir = os.path.dirname(os.path.abspath(base_file))
pr_file = os.path.join(sources, "lakefile.toml")
errors = []
# Lake prefers lakefile.lean, a Lean program, over lakefile.toml.
for side, d in (("base", base_dir), ("pull request", sources)):
    if os.path.lexists(os.path.join(d, "lakefile.lean")):
        errors.append(f"the {side} has a lakefile.lean; only lakefile.toml is accepted")
for side, f in (("base", base_file), ("pull request", pr_file)):
    if not os.path.isfile(f) or os.path.islink(f):
        errors.append(f"the {side} lakefile.toml is not a regular file")
if errors:
    for e in errors:
        print(f"FAIL: {e}", file=sys.stderr)
    sys.exit(1)

# The keys that decide which Lean sources Lake compiles (Lake v4.35.0:
# PackageConfig.srcDir; LeanLibConfig name/srcDir/roots/globs; LeanExeConfig
# name/srcDir/root). Lake matches TOML keys exactly, and so does this.
TARGET_KEYS = ("name", "srcDir", "roots", "globs", "root")
ABSENT = "<absent>"


def layout(path, side):
    try:
        with open(path, "rb") as fh:
            doc = tomllib.load(fh)
    except (OSError, ValueError) as e:
        sys.exit(f"FAIL: cannot parse the {side} lakefile.toml: {e}")
    out = {"srcDir": json.dumps(doc.get("srcDir", ABSENT))}
    for kind in ("lean_lib", "lean_exe"):
        tables = doc.get(kind, [])
        if not isinstance(tables, list) or not all(isinstance(t, dict) for t in tables):
            sys.exit(f"FAIL: the {side} lakefile.toml has a {kind} that is "
                     "not an array of tables")
        out[kind] = sorted(
            json.dumps({k: t.get(k, ABSENT) for k in TARGET_KEYS}, sort_keys=True)
            for t in tables)
    return out


base = layout(base_file, "base")
pr = layout(pr_file, "pull request")
if base != pr:
    print("FAIL: lakefile.toml changes which sources are compiled (package "
          "srcDir, or a lean_lib/lean_exe name, srcDir, roots, globs or root)",
          file=sys.stderr)
    for key in ("srcDir", "lean_lib", "lean_exe"):
        if base[key] != pr[key]:
            print(f"  {key}: base {base[key]}; pull request {pr[key]}", file=sys.stderr)
    sys.exit(1)
print(f"  same layout as the base: srcDir {base['srcDir']}, "
      f"{len(base['lean_lib'])} lean_lib, {len(base['lean_exe'])} lean_exe")
PY
then
  echo "FAIL: source layout" >&2
  exit 1
fi

if [[ "$scan" == 1 ]]; then
  echo "== verify: source scan (base check-sorry.sh --no-build over $sources)"
  # LAKE=false: the scan must not build; if a future check-sorry.sh tried to,
  # it fails closed instead of compiling model code here.
  if ! LAKE=false bash "$here/check-sorry.sh" --no-build "$sources"; then
    echo "FAIL: source scan rejected the pull request sources" >&2
    exit 1
  fi
fi

echo "== verify: artifact intake ($lib)"
if ! python3 -I - "$lib" "$tmp/lib" "$sources" "$gate_lib" \
  >"$tmp/modules" <<'PY'
import os
import re
import shutil
import stat
import sys

src_lib, dst_lib, sources, lib = sys.argv[1:]
EXTS = (".olean", ".olean.server", ".olean.private", ".ir")
COMP = re.compile(r"^[A-Za-z_][A-Za-z0-9_']*$")
errors = []


def module_of(rel, exts):
    """`Botho/A/B.olean` -> `Botho.A.B`, or None if not a library path."""
    for ext in exts:
        if rel.endswith(ext):
            parts = rel[: -len(ext)].split("/")
            if parts[0] == lib and all(COMP.match(p) for p in parts):
                return ".".join(parts)
    return None


olean_mods = set()
for dirpath, dirnames, filenames in os.walk(src_lib):
    for name in sorted(dirnames + filenames):
        path = os.path.join(dirpath, name)
        rel = os.path.relpath(path, src_lib).replace(os.sep, "/")
        st = os.lstat(path)
        if stat.S_ISDIR(st.st_mode):
            if not (rel == lib or rel.startswith(lib + "/")):
                errors.append(f"unexpected directory in artifact: {rel}")
            elif not all(COMP.match(p) for p in rel.split("/")):
                errors.append(f"unexpected directory name in artifact: {rel}")
            continue
        if not stat.S_ISREG(st.st_mode):
            errors.append(f"not a regular file in artifact: {rel}")
            continue
        mod = module_of(rel, EXTS)
        if mod is None:
            errors.append(f"unexpected file in artifact: {rel}")
            continue
        if rel.endswith(".olean"):
            olean_mods.add(mod)
        dst = os.path.join(dst_lib, rel)
        os.makedirs(os.path.dirname(dst), exist_ok=True)
        shutil.copyfile(path, dst)

src_mods = set()
if os.path.isfile(os.path.join(sources, lib + ".lean")):
    src_mods.add(lib)
for dirpath, _, filenames in os.walk(os.path.join(sources, lib)):
    for name in filenames:
        if name.endswith(".lean"):
            rel = os.path.relpath(os.path.join(dirpath, name), sources)
            mod = module_of(rel.replace(os.sep, "/"), (".lean",))
            if mod is None:
                errors.append(f"source file with an invalid module name: {rel}")
            else:
                src_mods.add(mod)

if lib not in olean_mods:
    errors.append(f"artifact has no {lib}.olean")
if not src_mods:
    errors.append(f"no {lib}.lean or {lib}/**/*.lean in the sources")
for m in sorted(olean_mods - src_mods):
    errors.append(f"compiled module {m} has no source file {m.replace('.', '/')}.lean")
for m in sorted(src_mods - olean_mods):
    errors.append(f"source module {m} has no .olean in the artifact")
if errors:
    for e in errors:
        print(f"FAIL: {e}", file=sys.stderr)
    sys.exit(1)
print("\n".join(sorted(olean_mods)))
PY
then
  echo "FAIL: artifact intake" >&2
  exit 1
fi
echo "  $(wc -l <"$tmp/modules" | tr -d ' ') modules"

echo "== verify: manifest (Theorems.lean, parsed as text)"
if ! python3 -I - "$sources/Theorems.lean" >"$tmp/names" <<'PY'
import re
import sys

path = sys.argv[1]
with open(path, encoding="utf-8") as fh:
    src = fh.read()


def blank_comments(s):
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
    return "".join(out)


PRINT = re.compile(r"^\s*#print\s+axioms\s+(\S+)\s*$")
names = []
for line in blank_comments(src).split("\n"):
    m = PRINT.match(line)
    if m:
        name = m.group(1)
        while name.startswith("_root_."):
            name = name[len("_root_."):]
        if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_'!?]*(\.[A-Za-z_][A-Za-z0-9_'!?]*)*", name):
            sys.exit(f"FAIL: {path}: unsupported theorem name {name!r}")
        names.append(name)
if not names:
    sys.exit(f"FAIL: {path}: no '#print axioms' lines (empty manifest)")
print("\n".join(dict.fromkeys(names)))
PY
then
  echo "FAIL: manifest" >&2
  exit 1
fi
local names=()
while IFS= read -r f; do names+=("$f"); done <"$tmp/names"
echo "  ${#names[@]} theorems"

# Lean runs from an empty directory with only the copied artifact on
# LEAN_PATH. The toolchain's own library comes from LEAN_SYSROOT, set to
# --sysroot on each call, never from a `lean` on PATH.
echo "== verify: leanchecker $gate_lib (kernel replay)"
if ! (cd "$tmp/cwd" && LEAN_SYSROOT="$sysroot" LEAN_PATH="$tmp/lib" \
  "$checker" "$gate_lib"); then
  echo "FAIL: leanchecker rejected a declaration in $gate_lib" >&2
  exit 1
fi

echo "== verify: independent axiom walk (AxiomReport.lean, extensions not loaded)"
cp "$here/AxiomReport.lean" "$tmp/AxiomReport.lean"
: >"$tmp/report"
if ! (cd "$tmp/cwd" && LEAN_SYSROOT="$sysroot" LEAN_PATH="$tmp/lib" \
  "$lean" --run "$tmp/AxiomReport.lean" \
  "$tmp/report" "$gate_lib" "${names[@]}"); then
  echo "FAIL: AxiomReport.lean failed" >&2
  exit 1
fi

python3 -I - "$tmp/report" "$allowlist" "$tmp/names" "$tmp/modules" "$gate_lib" <<'PY'
import sys

report, allowlist, names_file, modules_file, lib = sys.argv[1:]
allowed = set()
with open(allowlist, encoding="utf-8") as fh:
    for row in fh:
        name = row.split("#", 1)[0].strip()
        if name:
            allowed.add(name)
with open(names_file, encoding="utf-8") as fh:
    order = [n for n in fh.read().split("\n") if n]
with open(modules_file, encoding="utf-8") as fh:
    files = {m for m in fh.read().split("\n") if m}
with open(report, encoding="utf-8") as fh:
    rows = [r.split("\t") for r in fh.read().split("\n") if r]

errors = []
if not rows or rows[-1][0] != "DONE":
    sys.exit("FAIL: AxiomReport.lean wrote an incomplete report")
swept = int(rows[-1][1])
if swept <= 0:
    errors.append("the walk swept no library declarations")

walked, modules = {}, set()


def import_ok(m):
    return m == "Init" or m.startswith("Init.") or m == lib or m.startswith(lib + ".")


for row in rows[:-1]:
    kind = row[0]
    if kind == "AXIOMS":
        walked[row[1]] = [a for a in row[2].split(",") if a]
    elif kind == "MISSING":
        errors.append(f"{row[1]} is not a declaration of the compiled library")
    elif kind == "SORRY":
        errors.append(f"{row[1]} reaches sorryAx (a sorry is reachable)")
    elif kind == "MODULE":
        modules.add(row[1])
    elif kind == "IMPORT":
        if not import_ok(row[2]):
            errors.append(f"module {row[1]} imports {row[2]} (only Init and {lib} modules)")
    elif kind == "AXIOMDECL":
        if row[1] not in allowed:
            errors.append(f"the library declares non-allow-listed axiom {row[1]}")
    elif kind == "INIT":
        errors.append(f"module {row[1]} declares an initializer ({row[2]})")
    else:
        errors.append(f"unknown report record {kind}")

for m in sorted(modules ^ files):
    side = "compiled module" if m in modules else "artifact file"
    errors.append(f"{side} {m} is not in both the import closure and the artifact")

for name in order:
    for ax in walked.get(name, []):
        if ax == "sorryAx":
            errors.append(f"{name} depends on sorryAx (a sorry is reachable)")
        elif ax not in allowed:
            errors.append(f"{name} depends on non-allow-listed axiom {ax}")

if errors:
    for e in errors:
        print(f"FAIL: {e}", file=sys.stderr)
    sys.exit(1)
for name in order:
    print(f"  {name}: {', '.join(walked[name]) or 'no axioms'}")
print(
    f"OK: verify passed ({len(order)} theorems; {len(modules)} modules; "
    f"{swept} library declarations swept; allow-list: {', '.join(sorted(allowed))})"
)
PY
}

main "$@"
