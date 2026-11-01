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

# The build layout (#1770 review; narrowed to the layout keys in #1799).
# Lake compiles whatever the lakefile says, so the gates accept one layout:
# one `Botho` library rooted at Botho.lean with sources in Botho/, and no
# executables. Only the keys that decide which sources Lake compiles are
# checked, the same keys as verify-artifacts.sh step 1 (Lake v4.35.0: the
# package `srcDir`; `name`, `srcDir`, `roots`, `globs` and `root` of every
# [[lean_lib]] and [[lean_exe]]). Other keys (version, options, [[require]]
# pins) may change, as the verify job allows.
#
# This used to be an exact-text pin of lakefile.toml, lean-toolchain and
# lake-manifest.json. The verify job (#1799) runs the base branch's
# check-sorry.sh over each pull request, so those pins would reject edits
# the verify job accepts on purpose (fixture lakefile-benign) and block every
# toolchain bump. The trust decisions they made are now the verify job's:
# it compares the layout with the base's lakefile, installs only official
# toolchain releases, and rejects any import outside Init and Botho in the
# compiled library.

# lakefile_preflight: fail unless the build configuration compiles exactly
# the scanned sources.
#   - lakefile.lean is a Lean program that lake runs on every `lake` call.
#   - A lakefile.toml whose layout keys differ from EXPECTED below could
#     move the compiled sources out of the scan (srcDir, roots, globs, a
#     second lean_lib or a lean_exe).
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
import json, sys
try:
    import tomllib
except ImportError:
    sys.exit("FAIL: python3 has no tomllib (needs Python 3.11 or later)")
KEYS = ("name", "srcDir", "roots", "globs", "root")
ABSENT = "<absent>"
# The pinned layout: the package has no srcDir, one lean_lib named Botho
# with no srcDir, roots, globs or root, and no lean_exe.
EXPECTED = {"lean_lib": [{"name": "Botho"}], "lean_exe": []}


def layout(doc):
    out = {"srcDir": json.dumps(doc.get("srcDir", ABSENT))}
    for kind in ("lean_lib", "lean_exe"):
        tables = doc.get(kind, [])
        if not isinstance(tables, list) or not all(isinstance(t, dict) for t in tables):
            sys.exit(f"FAIL: lakefile.toml has a {kind} that is not an array of tables")
        out[kind] = sorted(
            json.dumps({k: t.get(k, ABSENT) for k in KEYS}, sort_keys=True)
            for t in tables)
    return out


try:
    with open(sys.argv[1], "rb") as fh:
        doc = tomllib.load(fh)
except (OSError, ValueError) as e:
    sys.exit(f"FAIL: cannot parse lakefile.toml: {e}")
got, want = layout(doc), layout(EXPECTED)
if got != want:
    for key in ("srcDir", "lean_lib", "lean_exe"):
        if got[key] != want[key]:
            print(f"  {key}: expected {want[key]}; found {got[key]}", file=sys.stderr)
    sys.exit(1)
' "$root/lakefile.toml"; then
    echo "FAIL: lakefile.toml changes the build layout (package srcDir, or a lean_lib/lean_exe name, srcDir, roots, globs or root; see gate-common.sh)" >&2
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
