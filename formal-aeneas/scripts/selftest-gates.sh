#!/usr/bin/env bash
# Self-test for the formal-aeneas gates (#1821): each injected defect must be
# rejected, and the clean tree must pass.
#
# Part 1 (fast, no Lean): the source scan rejects `sorry`, `axiom` in every
# spelling the #1822 review listed (line start, indented, `private`,
# attribute-prefixed), `opaque`, a code-substituting attribute, a foreign
# import and an extra lean_lib; and `gate_scan.py --generated`, which
# regen.sh uses, rejects them in an extraction output directory too.
#
# Part 2 (needs the built package, unless --scan-only): with the scan
# switched off (`check-gates.sh --no-scan`), the axiom walk and the
# `#print axioms` cross-check still reject a `sorry`, an `axiom`, an
# `opaque` and a manifest entry that names no declaration. Each case builds a
# copy of the package that shares `.lake/packages` with ROOT (no download).
#
# Usage: selftest-gates.sh [--scan-only] [ROOT]
# Environment: LAKE overrides the lake binary (default: lake).
# Backquoted Lean names in single-quoted patterns are literal on purpose.
# shellcheck disable=SC2016
set -euo pipefail

scan_only=0
if [[ "${1:-}" == "--scan-only" ]]; then scan_only=1; shift; fi
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "${1:-$here/..}" && pwd)"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT
fails=0

# Copy the package sources (not .lake) into a fresh directory $1.
fresh() {
  local d="$1"
  rm -rf "$d"; mkdir -p "$d"
  cp -R "$root/generated" "$root/BothoAeneas" "$d/"
  cp "$root/BothoAeneas.lean" "$root/Theorems.lean" "$root/axioms-allowlist.txt" \
     "$root/lakefile.toml" "$root/lake-manifest.json" "$root/lean-toolchain" \
     "$root/toolchain.env" "$d/"
}

# expect pass|fail|fail:PATTERN LABEL CMD...
# With fail:PATTERN the output must also contain PATTERN (a fixed string), so
# that a case cannot pass by failing for an unrelated reason.
expect() {
  local want="$1" label="$2"; shift 2
  local pattern=""
  if [[ "$want" == fail:* ]]; then pattern="${want#fail:}"; want=fail; fi
  local got=pass
  "$@" >"$tmp/log" 2>&1 || got=fail
  if [[ "$got" == fail && -n "$pattern" ]] && ! grep -qF -- "$pattern" "$tmp/log"; then
    got="fail without '$pattern'"
  fi
  if [[ "$got" == "$want" ]]; then
    echo "ok   - $label ($got)"
  else
    echo "FAIL - $label: expected $want, got $got"; sed 's/^/       /' "$tmp/log"
    fails=$((fails + 1))
  fi
}

scan() { python3 -I "$here/gate_scan.py" "$@"; }
gen="generated/MonetaryKernels.lean"
proof="BothoAeneas/HalvingReward.lean"

echo "== part 1: source scan"
d="$tmp/s"
fresh "$d"; expect pass "clean tree" scan "$d"
fresh "$d"; printf '\ntheorem t : 1 = 2 := sorry\n' >>"$d/$proof"
expect 'fail:forbidden token `sorry`' "sorry in a proof" scan "$d"
fresh "$d"; printf '\ntheorem t : 1 = 2 := by admit\n' >>"$d/$proof"
expect 'fail:forbidden token `admit`' "admit in a proof" scan "$d"
fresh "$d"; printf '\naxiom bad : False\n' >>"$d/$gen"
expect 'fail:forbidden token `axiom`' "axiom at line start (generated)" scan "$d"
fresh "$d"; printf '\n  axiom bad : False\n' >>"$d/$gen"
expect 'fail:forbidden token `axiom`' "indented axiom (generated)" scan "$d"
fresh "$d"; printf '\nprivate axiom bad : False\n' >>"$d/$gen"
expect 'fail:forbidden token `axiom`' "private axiom (generated)" scan "$d"
fresh "$d"; printf '\n@[simp] axiom bad : False\n' >>"$d/$gen"
expect 'fail:forbidden token `axiom`' "attribute-prefixed axiom (generated)" scan "$d"
fresh "$d"; printf '\n@[irreducible]\naxiom bad : False\n' >>"$d/$proof"
expect 'fail:forbidden token `axiom`' "attribute on the line above an axiom (proof)" scan "$d"
fresh "$d"; printf '\nopaque o : Nat\n' >>"$d/$gen"
expect 'fail:forbidden token `opaque`' "opaque (generated)" scan "$d"
fresh "$d"; printf '\n@[implemented_by id] def f (n : Nat) : Nat := n\n' >>"$d/$proof"
expect 'fail:forbidden attribute `implemented_by`' "implemented_by attribute" scan "$d"
fresh "$d"; printf '\ntheorem t : 2 + 2 = 4 := by native_decide\n' >>"$d/$proof"
expect 'fail:forbidden token `native_decide`' "native_decide" scan "$d"
fresh "$d"; printf '\nset_option debug.skipKernelTC true\n' >>"$d/$proof"
expect 'fail:forbidden token `skipKernelTC`' "debug.skipKernelTC" scan "$d"
fresh "$d"; { echo "import Mathlib"; cat "$root/$proof"; } >"$d/$proof"
expect 'fail:import of `Mathlib`' "import outside the allow-list" scan "$d"
fresh "$d"; printf '\n[[lean_lib]]\nname = "Extra"\n' >>"$d/lakefile.toml"
expect 'fail:libraries must be' "extra lean_lib" scan "$d"
fresh "$d"; sed 's/^AENEAS_COMMIT=.*/AENEAS_COMMIT=0000000000000000000000000000000000000000/' \
  "$root/toolchain.env" >"$d/toolchain.env"
expect 'fail:require must be exactly' "Lean library commit differs from AENEAS_COMMIT" scan "$d"
fresh "$d"; printf 'def x := 1\n' >"$d/Stray.lean"
expect 'fail:outside the scanned set' "stray .lean file" scan "$d"
fresh "$d"; printf '\n-- axiom in a comment is fine\n/- so is sorry -/\n' >>"$d/$proof"
expect pass "axiom/sorry only in comments" scan "$d"
# regen.sh's check is `gate_scan.py --generated <extraction output dir>`.
g="$tmp/g"
rm -rf "$g"; mkdir -p "$g"; cp "$root/$gen" "$g/"
expect pass "--generated: clean extraction" scan --generated "$g"
printf '\n@[reducible] axiom core.num.U128.saturating_mul : U128 → U128 → Result U128\n' >>"$g/MonetaryKernels.lean"
expect 'fail:forbidden token `axiom`' "--generated: attribute-prefixed axiom" scan --generated "$g"
cp "$root/$gen" "$g/"; printf '\nopaque F64 : Type\n' >>"$g/MonetaryKernels.lean"
expect 'fail:forbidden token `opaque`' "--generated: opaque" scan --generated "$g"

if [[ "$scan_only" == 0 ]]; then
  echo "== part 2: axiom walk and #print axioms, with the scan off"
  if [[ ! -d "$root/.lake/packages/aeneas" ]]; then
    echo "FAIL: $root/.lake/packages is not populated; run lake build there first" >&2
    exit 1
  fi
  # walk_case LABEL PATTERN APPEND_TO_PROOF [MANIFEST_LINE]. The appended text
  # lands after the module's `end`, so its names are top-level.
  walk_case() {
    local d="$tmp/w"
    fresh "$d"
    mkdir -p "$d/.lake"
    ln -s "$root/.lake/packages" "$d/.lake/packages"
    printf '%s\n' "$3" >>"$d/$proof"
    if [[ -n "${4:-}" ]]; then printf '%s\n' "$4" >>"$d/Theorems.lean"; fi
    expect "fail:$2" "$1" "$here/check-gates.sh" --no-scan "$d"
  }
  walk_case "walk: sorry" 'selftest_sorry depends on sorryAx' \
    'theorem selftest_sorry : (1 : Nat) = 2 := sorry'
  walk_case "walk: axiom used by a listed theorem" \
    "selftest_uses depends on axioms outside the allow-list: ['selftest_ax']" \
    $'axiom selftest_ax : (1 : Nat) = 2\ntheorem selftest_uses : (1 : Nat) = 2 := selftest_ax' \
    '#print axioms selftest_uses'
  walk_case "walk: unused @[simp] axiom" 'selftest_ax2: `axiom` declared in the package' \
    '@[simp] axiom selftest_ax2 : (3 : Nat) = 4'
  walk_case "walk: opaque" 'selftest_opaque: `opaque` declared in the package' \
    'opaque selftest_opaque : Nat'
  walk_case "walk: manifest names a missing theorem" \
    'no_such_theorem: listed in Theorems.lean but not a declaration' \
    '' '#print axioms BothoAeneas.HalvingReward.no_such_theorem'
fi

if [[ "$fails" -ne 0 ]]; then
  echo "FAIL: $fails self-test case(s) failed" >&2
  exit 1
fi
echo "OK: all gate self-tests passed"
