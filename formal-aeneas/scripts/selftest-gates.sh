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
evidence="${SELFTEST_EVIDENCE_DIR:-}"
if [[ -n "$evidence" ]]; then mkdir -p "$evidence"; fi

# Copy the package sources (not .lake) into a fresh directory $1.
fresh() {
  local d="$1"
  rm -rf "$(dirname "${d:?}")"; mkdir -p "$d" "$d/../formal/Botho/Lottery" "$d/../botho/src/consensus"
  local module
  for module in Pool Draw Prims Candidate Arith; do
    cp "$root/../formal/Botho/Lottery/$module.lean" "$d/../formal/Botho/Lottery/"
  done
  cp "$root/../formal/lean-toolchain" "$d/../formal/"
  cp "$root/../botho/src/consensus/lottery.rs" "$d/../botho/src/consensus/"
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
  if [[ -n "$evidence" ]]; then
    cp "$tmp/log" "$evidence/$(printf '%s' "$label" | tr -cs '[:alnum:]' '_').log"
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
d="$tmp/s/formal-aeneas"
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

echo "== canonical closure and wrapper source controls"
canonical="../formal/Botho/Lottery"
for module in Pool Draw Prims Candidate Arith; do
  fresh "$d"; printf '\naxiom canonical_bad : False\n' >>"$d/$canonical/$module.lean"
  expect 'fail:forbidden token `axiom`' "canonical axiom: $module" scan "$d"
done
fresh "$d"; printf '\nimport Botho.Lottery.Conservation\n' >>"$d/$canonical/Pool.lean"
expect 'fail:import of `Botho.Lottery.Conservation`' "canonical extra import" scan "$d"
fresh "$d"; printf '\n@[implemented_by id] def bad (n : Nat) := n\n' >>"$d/$canonical/Arith.lean"
expect 'fail:forbidden attribute `implemented_by`' "canonical native substitution" scan "$d"
fresh "$d"; rm "$d/$canonical/Arith.lean"
expect 'fail:canonical source must be a regular file' "missing canonical source" scan "$d"
fresh "$d"; mv "$d/$canonical/Arith.lean" "$d/../arith-copy"; ln -s ../../arith-copy "$d/$canonical/Arith.lean"
expect 'fail:canonical source must be a regular file' "canonical source symlink" scan "$d"
fresh "$d"; mv "$d/../formal" "$d/../formal-copy"; ln -s formal-copy "$d/../formal"
expect 'fail:canonical directory must be regular' "canonical directory symlink" scan "$d"
fresh "$d"; mkdir -p "$d/Botho/Lottery"; cp "$root/../formal/Botho/Lottery/Arith.lean" "$d/Botho/Lottery/"
expect 'fail:outside the scanned set' "shadow canonical source" scan "$d"
fresh "$d"; echo leanprover/lean4:v0.0.0 >"$d/../formal/lean-toolchain"
expect 'fail:canonical Lean toolchain: pins differ' "canonical toolchain mismatch" scan "$d"
for mutation in layout root glob extra; do
  fresh "$d"
  python3 - "$d/lakefile.toml" "$mutation" <<'PYCONFIG'
from pathlib import Path
import sys
p = Path(sys.argv[1]); s = p.read_text(); kind = sys.argv[2]
if kind == "layout": s = s.replace('srcDir = "../formal"', 'srcDir = "../formal/.lake"')
if kind == "root": s = s.replace('roots = ["Botho.Lottery.Pool",', 'roots = [')
if kind == "glob": s = s.replace('globs = ["Botho.Lottery.Pool",', 'globs = ["Botho.Lottery.*",')
if kind == "extra": s = s.replace('roots = [', 'roots = ["Botho.Basic",')
p.write_text(s)
PYCONFIG
  expect 'fail:canonical library must use the exact reviewed source closure' "canonical library $mutation" scan "$d"
done
for mutation in arguments field conditional duplicate literal unsupported nested; do
  fresh "$d"
  python3 - "$d/../botho/src/consensus/lottery.rs" "$mutation" <<'PYWRAPPER'
from pathlib import Path
import sys
p = Path(sys.argv[1]); s = p.read_text(); kind = sys.argv[2]
start = s.index('pub fn compute_pool_accounting(')
end = s.index('\n}', start) + 2
old = s[start:end]; new = old
if kind == "arguments": new = old.replace('        total_fees,\n        emission_share,', '        emission_share,\n        total_fees,')
if kind == "field": new = old.replace('config.pool_fraction_permille,', '800,')
if kind == "conditional": new = old.replace('    compute_pool_accounting_permille(', '    if total_fees == 0 { panic!("bad") }\n    compute_pool_accounting_permille(')
if kind == "duplicate": new = old + '\n' + old
if kind == "literal": new = 'const FAKE: &str = r###"' + old + '"###; /* ' + old + ' */'
if kind == "nested": new = '#[cfg(any())] mod hidden { fn before() {}\n' + old + '\n}'
if kind == "unsupported": new = old.replace('    compute_pool_accounting_permille(', '    return compute_pool_accounting_permille(')
assert new != old
p.write_text(s[:start] + new + s[end:])
PYWRAPPER
  expect 'fail:wrapper source contract:' "wrapper $mutation" scan "$d"
done

if [[ "$scan_only" == 0 ]]; then
  echo "== part 2: axiom walk and #print axioms, with the scan off"
  if [[ ! -d "$root/.lake/packages/aeneas" ]]; then
    echo "FAIL: $root/.lake/packages is not populated; run lake build there first" >&2
    exit 1
  fi
  # walk_case LABEL PATTERN APPEND_TO_PROOF [MANIFEST_LINE]. The appended text
  # lands after the module's `end`, so its names are top-level.
  walk_case() {
    local d="$tmp/w/formal-aeneas"
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
  # The canonical module is owned even when its new axiom is unused.
  d="$tmp/canonical-walk/formal-aeneas"; fresh "$d"
  mkdir -p "$d/.lake"; ln -s "$root/.lake/packages" "$d/.lake/packages"
  printf '\naxiom canonical_unused : Nat\n' >>"$d/$canonical/Arith.lean"
  expect 'fail:canonical_unused: `axiom` declared in the package' \
    "walk: unused canonical axiom" "$here/check-gates.sh" --no-scan "$d"

  # Compile clean sources first, mutate in place (warm artifacts), then restore.
  # The first actual rejecting declaration must be the new model bridge.
  for mutation in Pool Arith; do
    d="$tmp/canonical-proof/formal-aeneas"; fresh "$d"
    mkdir -p "$d/.lake"; ln -s "$root/.lake/packages" "$d/.lake/packages"
    expect pass "canonical $mutation warm baseline" "$here/check-gates.sh" "$d"
    cp "$d/$canonical/$mutation.lean" "$tmp/original.lean"
    python3 - "$d/$canonical/$mutation.lean" "$mutation" <<'PYMUTATE'
from pathlib import Path
import sys
p = Path(sys.argv[1]); s = p.read_text()
old, new = (("payout := minN available payoutCap", "payout := minN available payoutCap + 1")
            if sys.argv[2] == "Pool" else
            ("if a + b ≤ max then a + b else max", "if a + b ≤ max then a + b else max + 1"))
assert s.count(old) == 1
p.write_text(s.replace(old, new))
PYMUTATE
    if [[ -n "$evidence" ]]; then
      diff -u "$tmp/original.lean" "$d/$canonical/$mutation.lean" >"$evidence/$mutation.patch" || true
      sha256sum "$d/$canonical/$mutation.lean" >"$evidence/$mutation-mutant.sha256"
    fi
    expect 'fail:FAIL: lake build failed' "canonical $mutation mutation" "$here/check-gates.sh" "$d"
    # Fail closed if an unrelated compilation error rejected the mutant.
    python3 - "$tmp/log" "$d/BothoAeneas/PoolAccounting.lean" <<'PYREJECT' || fails=$((fails + 1))
from pathlib import Path
import re, sys
log, source = Path(sys.argv[1]).read_text(), Path(sys.argv[2]).read_text().splitlines()
m = re.search(r"error: (?:[^\n]*?/)?BothoAeneas/PoolAccounting.lean:(\d+):", log)
if not m: raise SystemExit("FAIL: no bridge compiler diagnostic")
line = int(m.group(1)); declarations = []
for n, text in enumerate(source[:line], 1):
    if match := re.match(r"theorem (\w+)", text): declarations.append((n, match.group(1)))
actual = declarations[-1][1]
print("first rejecting declaration:", actual, "line", line)
if actual != "compute_pool_accounting_permille_model": raise SystemExit("FAIL: wrong rejecting declaration")
PYREJECT
    cp "$tmp/original.lean" "$d/$canonical/$mutation.lean"
    cmp "$root/../formal/Botho/Lottery/$mutation.lean" "$d/$canonical/$mutation.lean"
    expect pass "canonical $mutation restored" "$here/check-gates.sh" "$d"
  done
  walk_case "walk: manifest names a missing theorem" \
    'no_such_theorem: listed in Theorems.lean but not a declaration' \
    '' '#print axioms BothoAeneas.HalvingReward.no_such_theorem'
fi

if [[ "$fails" -ne 0 ]]; then
  echo "FAIL: $fails self-test case(s) failed" >&2
  exit 1
fi
echo "OK: all gate self-tests passed"
