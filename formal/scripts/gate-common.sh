# shellcheck shell=bash
# Shared helpers for the formal gates (#1770). Source this file; do not run it.
#
# Callers set: here (this directory), root (the formal/ tree), lake (the lake
# binary) and gate_lib (the library to check), and source this file before
# their first `lake` call. Callers set tmp (a private scratch directory)
# before calling run_report.
: "${here:?}" "${root:?}" "${lake:?}" "${gate_lib:?}"

# The report program, read now: before any `lake` call, so model code that
# runs during `lake build` cannot rewrite what the gate runs (#1770 review).
# run_report writes this copy out just before Lean reads it.
if [[ ! -f "$here/AxiomReport.lean" ]]; then
  echo "FAIL: missing $here/AxiomReport.lean" >&2
  exit 1
fi
gate_report_src="$(cat "$here/AxiomReport.lean")"

# The build configuration (#1770 review; narrowed to the layout keys in
# #1799; an explicit key allow-list again after the #1783 review).
# Lake compiles whatever the lakefile says, and some lakefile keys load or
# run code, so the gates accept one shape of lakefile.toml:
#
#   - Layout: one `Botho` library rooted at Botho.lean with sources in
#     Botho/, and no executables. The keys that decide which sources Lake
#     compiles are the same keys as verify-artifacts.sh step 1 (Lake v4.35.0:
#     the package `srcDir`; `name`, `srcDir`, `roots`, `globs` and `root` of
#     every [[lean_lib]] and [[lean_exe]]), and they must match EXPECTED.
#   - Every other key must be on an allow-list of keys that cannot load or
#     run code: package metadata (`name`, `version`, `defaultTargets`,
#     `description`, `keywords`, `homepage`, `license`) and `leanOptions`
#     with plain values (bool, integer or string) for a short list of
#     options (OPTION_NAMES, OPTION_PREFIXES). A [[lean_lib]] may carry only
#     its `name`. Everything else fails, known or not: `[[require]]` (a
#     dependency's lakefile.lean is a Lean program that Lake elaborates, and
#     runs any `#eval` in, on every `lake` call, and its sources are never
#     scanned), `moreLeanArgs`, `weakLeanArgs`, `moreServerOptions`,
#     `moreLinkArgs`, `plugins`, `dynlibs`, `precompileModules`,
#     `extraDepTargets`, `buildDir`, `testDriver`, ... An option outside the
#     list fails too, since `debug.skipKernelTC = true` in `leanOptions`
#     would switch off the kernel without the scan ever seeing it.
#   - lake-manifest.json, when present, must be a regular JSON file with an
#     empty `packages` list (a second layer behind the `[[require]]` ban).
#
# This used to be an exact-text pin of lakefile.toml, lean-toolchain and
# lake-manifest.json. The verify job (#1799) runs the base branch's
# check-sorry.sh over each pull request, so those pins would reject edits
# the verify job accepts on purpose (fixture lakefile-benign) and block every
# toolchain bump. The allow-list accepts those edits. The toolchain is still
# the verify job's decision: it installs only official releases.

# lakefile_preflight: fail unless the build configuration compiles exactly
# the scanned sources and loads no other code.
#   - lakefile.lean is a Lean program that lake runs on every `lake` call.
#   - lakefile.toml must pass the layout and key allow-list above.
lakefile_preflight() {
  local f
  if [[ -e "$root/lakefile.lean" || -L "$root/lakefile.lean" ]]; then
    echo "FAIL: $root/lakefile.lean is not allowed (use lakefile.toml)" >&2
    return 1
  fi
  for f in lakefile.toml lean-toolchain; do
    if [[ -L "$root/$f" || ! -f "$root/$f" ]]; then
      echo "FAIL: $root/$f must be a regular file (not missing, not a symlink)" >&2
      return 1
    fi
  done
  if ! python3 -I -c '
import json, os, sys
try:
    import tomllib
except ImportError:
    sys.exit("FAIL: python3 has no tomllib (needs Python 3.11 or later)")
LAYOUT_KEYS = ("name", "srcDir", "roots", "globs", "root")
ABSENT = "<absent>"
# The pinned layout: the package has no srcDir, one lean_lib named Botho
# with no srcDir, roots, globs or root, and no lean_exe.
EXPECTED = {"lean_lib": [{"name": "Botho"}], "lean_exe": []}
# Top-level keys that cannot load or run code, with their TOML types. The
# layout keys (srcDir, lean_lib, lean_exe) are allowed here and pinned by
# the layout check.
STR, STRS = "string", "array of strings"
PACKAGE_KEYS = {
    "name": STR, "version": STR, "description": STR, "homepage": STR,
    "license": STR, "keywords": STRS, "defaultTargets": STRS,
    "leanOptions": "table", "srcDir": STR, "lean_lib": "array of tables",
    "lean_exe": "array of tables",
}
# Keys a [[lean_lib]] may carry: its name and the (pinned) layout keys.
LIB_KEYS = ("name", "srcDir", "roots", "globs")
# leanOptions the model may set. Only options that change elaboration
# defaults, limits, linters or printing; nothing that turns off a check.
OPTION_NAMES = (
    "autoImplicit", "relaxedAutoImplicit", "maxHeartbeats", "maxRecDepth",
    "synthInstance.maxHeartbeats", "synthInstance.maxSize",
)
OPTION_PREFIXES = ("linter.", "pp.")


def typed(value, kind):
    if kind == STR:
        return isinstance(value, str)
    if kind == STRS:
        return isinstance(value, list) and all(isinstance(v, str) for v in value)
    if kind == "table":
        return isinstance(value, dict)
    return isinstance(value, list) and all(isinstance(v, dict) for v in value)


def options(table, prefix=""):
    """Flatten [leanOptions]: dotted TOML keys become nested tables."""
    for k, v in table.items():
        if isinstance(v, dict):
            yield from options(v, f"{prefix}{k}.")
        else:
            yield f"{prefix}{k}", v


def layout(doc):
    out = {"srcDir": json.dumps(doc.get("srcDir", ABSENT))}
    for kind in ("lean_lib", "lean_exe"):
        tables = doc.get(kind, [])
        out[kind] = sorted(
            json.dumps({k: t.get(k, ABSENT) for k in LAYOUT_KEYS}, sort_keys=True)
            for t in tables)
    return out


lakefile, manifest = sys.argv[1:]
try:
    with open(lakefile, "rb") as fh:
        doc = tomllib.load(fh)
except (OSError, ValueError) as e:
    sys.exit(f"FAIL: cannot parse lakefile.toml: {e}")
errors = []
for key, value in doc.items():
    kind = PACKAGE_KEYS.get(key)
    if kind is None:
        errors.append(f"key {key!r} is not allowed")
    elif not typed(value, kind):
        errors.append(f"key {key!r} must be a {kind}")
for key, value in options(doc.get("leanOptions", {}) if isinstance(doc.get("leanOptions"), dict) else {}):
    if not (key in OPTION_NAMES or key.startswith(OPTION_PREFIXES)):
        errors.append(f"leanOptions: option {key!r} is not allowed")
    elif not isinstance(value, (bool, int, str)):
        errors.append(f"leanOptions: option {key!r} must be a bool, integer or string")
for t in doc.get("lean_lib", []) if typed(doc.get("lean_lib", []), "array of tables") else []:
    for key in t:
        if key not in LIB_KEYS:
            errors.append(f"[[lean_lib]] key {key!r} is not allowed")
if not errors:
    got, want = layout(doc), layout(EXPECTED)
    if got != want:
        errors.append("lakefile.toml changes the build layout (package srcDir, or a "
                      "lean_lib/lean_exe name, srcDir, roots, globs or root)")
        for key in ("srcDir", "lean_lib", "lean_exe"):
            if got[key] != want[key]:
                errors.append(f"  {key}: expected {want[key]}; found {got[key]}")
# lake-manifest.json: optional, but if present it must list no packages.
if os.path.lexists(manifest):
    if os.path.islink(manifest) or not os.path.isfile(manifest):
        errors.append("lake-manifest.json must be a regular file (not a symlink)")
    else:
        try:
            with open(manifest, encoding="utf-8") as fh:
                m = json.load(fh)
        except (OSError, ValueError) as e:
            errors.append(f"cannot parse lake-manifest.json: {e}")
        else:
            pkgs = m.get("packages") if isinstance(m, dict) else None
            if pkgs != []:
                errors.append("lake-manifest.json must have an empty packages list "
                              f"(found {json.dumps(pkgs)[:200]})")
for e in errors:
    print(f"  {e}", file=sys.stderr)
sys.exit(1 if errors else 0)
' "$root/lakefile.toml" "$root/lake-manifest.json"; then
    echo "FAIL: lakefile.toml or lake-manifest.json is outside the allowed build configuration (layout, key allow-list, no dependencies; see gate-common.sh)" >&2
    return 1
  fi
}

# source_scan: run gate_scan.py (layout, import allow-list, token scan) on the
# tree. Prints the findings and returns 1 on failure. On success sets
# gate_scanned to the number of files scanned. Call it before any `lake` call.
source_scan() {
  local out
  if ! out="$(python3 -I "$here/gate_scan.py" "$root")"; then
    printf '%s\n' "$out"
    echo "FAIL: source scan rejected the model sources (see above)" >&2
    return 1
  fi
  # shellcheck disable=SC2034  # read by the calling gate
  gate_scanned="$(grep -c '^scanned: ' <<<"$out" || true)"
}

# run_report OUT [NAME ...]: run AxiomReport.lean (the copy read when this
# file was sourced) against the compiled library and write its records to
# OUT. Fails if Lean fails or the report is incomplete (no final DONE line).
run_report() {
  local out="$1" prog="${tmp:?}/AxiomReport.lean"
  shift
  : >"$out"
  printf '%s\n' "$gate_report_src" >"$prog"
  if ! (cd "$root" && "$lake" env lean --run "$prog" \
    "$out" "$gate_lib" "$@"); then
    echo "FAIL: AxiomReport.lean failed" >&2
    return 1
  fi
  if ! tail -n 1 "$out" | grep -Eq $'^DONE\t[1-9][0-9]*$'; then
    echo "FAIL: AxiomReport.lean wrote an incomplete report" >&2
    cat "$out" >&2
    return 1
  fi
}
