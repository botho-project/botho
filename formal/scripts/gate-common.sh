# shellcheck shell=bash
# Shared helpers for the formal gates (#1770). Source this file; do not run it.
#
# Callers set: here (this directory), root (the formal/ tree), lake (the lake
# binary) and gate_lib (the library to check).
: "${here:?}" "${root:?}" "${lake:?}" "${gate_lib:?}"

# lakefile_preflight: fail on build configuration that can run native code or
# switch off kernel checking outside the Lean sources the scan covers.
#   - lakefile.lean is a Lean program that lake runs on every `lake` call.
#   - plugins / dynlibs / extra lean args load or pass arbitrary native code
#     and options (for example `-Ddebug.skipKernelTC=true`).
lakefile_preflight() {
  if [[ -e "$root/lakefile.lean" ]]; then
    echo "FAIL: $root/lakefile.lean is not allowed (use lakefile.toml)" >&2
    return 1
  fi
  if [[ -f "$root/lakefile.toml" ]] \
    && grep -En '^[[:space:]]*(plugins|dynlibs|moreLeanArgs|weakLeanArgs|moreServerOptions)[[:space:]]*=|skipKernelTC|debug\.' \
      "$root/lakefile.toml" >&2; then
    echo "FAIL: lakefile.toml sets a forbidden build option (see above)" >&2
    return 1
  fi
}

# run_report OUT [NAME ...]: run AxiomReport.lean against the compiled
# library and write its records to OUT. Fails if Lean fails or the report is
# incomplete (no final DONE line).
run_report() {
  local out="$1"
  shift
  : >"$out"
  if ! (cd "$root" && "$lake" env lean --run "$here/AxiomReport.lean" \
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
