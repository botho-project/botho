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

# The build layout, pinned (#1770 review). Lake compiles whatever the lakefile
# says, so the gates accept exactly one layout: one `Botho` library rooted at
# Botho.lean with sources in Botho/, no dependencies, no extra libraries, no
# srcDir / roots / globs / extra arguments / plugins / native code. Any other
# lakefile.toml fails, which is simpler and safer than a list of banned keys.
# Changing the layout or bumping the toolchain means editing these pins in a
# reviewed diff.
#
# lakefile.toml is compared after dropping blank lines, full-line `#`
# comments, and leading and trailing whitespace.
gate_lakefile_pin='name = "botho_formal"
version = "0.1.0"
defaultTargets = ["Botho"]
[[lean_lib]]
name = "Botho"'
gate_toolchain_pin='leanprover/lean4:v4.35.0'
# lake-manifest.json, compared as parsed JSON: no packages.
gate_manifest_pin='{"version": "1.3.0", "packagesDir": ".lake/packages", "packages": [], "name": "botho_formal", "lakeDir": ".lake", "fixedToolchain": false}'

# lakefile_preflight: fail unless the build configuration is exactly the
# pinned layout above.
#   - lakefile.lean is a Lean program that lake runs on every `lake` call.
#   - Any lakefile.toml other than the pinned one could move the sources out
#     of the scan (srcDir, roots, globs, a second lean_lib, a [[require]]) or
#     load native code and options (plugins, moreLeanArgs, extern_lib, ...).
lakefile_preflight() {
  local f
  if [[ -e "$root/lakefile.lean" || -L "$root/lakefile.lean" ]]; then
    echo "FAIL: $root/lakefile.lean is not allowed (use lakefile.toml)" >&2
    return 1
  fi
  for f in lakefile.toml lean-toolchain lake-manifest.json; do
    if [[ -L "$root/$f" || ! -f "$root/$f" ]]; then
      echo "FAIL: $root/$f must be a regular file (not missing, not a symlink)" >&2
      return 1
    fi
  done
  local got
  got="$(sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' "$root/lakefile.toml" \
    | grep -v -e '^$' -e '^#' || true)"
  if [[ "$got" != "$gate_lakefile_pin" ]]; then
    echo "FAIL: lakefile.toml does not match the pinned layout (see gate-common.sh)" >&2
    diff <(printf '%s\n' "$gate_lakefile_pin") <(printf '%s\n' "$got") \
      | sed 's/^/  /' >&2 || true
    return 1
  fi
  got="$(sed -e 's/^[[:space:]]*//' -e 's/[[:space:]]*$//' "$root/lean-toolchain" \
    | grep -v -e '^$' || true)"
  if [[ "$got" != "$gate_toolchain_pin" ]]; then
    echo "FAIL: lean-toolchain is '$got', pinned '$gate_toolchain_pin' (see gate-common.sh)" >&2
    return 1
  fi
  if ! GATE_PIN="$gate_manifest_pin" python3 -I -c '
import json, os, sys
try:
    with open(sys.argv[1], encoding="utf-8") as fh:
        got = json.load(fh)
except (OSError, ValueError) as e:
    sys.exit(f"cannot parse: {e}")
sys.exit(0 if got == json.loads(os.environ["GATE_PIN"]) else 1)
' "$root/lake-manifest.json" >&2; then
    echo "FAIL: lake-manifest.json does not match the pinned manifest (see gate-common.sh)" >&2
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
