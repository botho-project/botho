#!/usr/bin/env bash
# Self-test for the formal gates (#1705; extended in #1724 and #1770).
#
# Copies the formal/ tree into scratch directories, injects a defect into each
# copy, and asserts that the matching gate rejects it. Also checks positive
# and negative controls, so a gate that always fails is caught too.
#
# Usage: selftest-gates.sh
# Environment: LAKE overrides the lake binary (default: lake).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
src="$(cd "$here/.." && pwd)"
scratch="$(mktemp -d)"
trap 'rm -rf "$scratch"' EXIT
failures=0

fresh_copy() {
  local dst
  dst="$(mktemp -d "$scratch/case.XXXXXX")"
  # Copy .lake too (when present) so each case reuses the existing build.
  (cd "$src" && tar cf - --exclude='./scripts' .) | (cd "$dst" && tar xf -)
  echo "$dst"
}

expect() {
  local want="$1" desc="$2"
  shift 2
  local log="$scratch/last.log" got
  if "$@" >"$log" 2>&1; then got=pass; else got=fail; fi
  if [[ "$got" == "$want" ]]; then
    echo "ok   - $desc (gate: $got)"
  else
    echo "FAIL - $desc (wanted gate to $want, got $got)"
    sed 's/^/       | /' "$log"
    failures=$((failures + 1))
  fi
}

# expect_fail_with PATTERN DESC CMD...: the gate must fail, and its output
# must match the extended regex PATTERN (so it fails for the intended reason,
# not an incidental one).
expect_fail_with() {
  local pattern="$1" desc="$2"
  shift 2
  local log="$scratch/last.log"
  if "$@" >"$log" 2>&1; then
    echo "FAIL - $desc (wanted gate to fail, got pass)"
    sed 's/^/       | /' "$log"
    failures=$((failures + 1))
  elif ! grep -Eq -- "$pattern" "$log"; then
    echo "FAIL - $desc (gate failed, but output does not match /$pattern/)"
    sed 's/^/       | /' "$log"
    failures=$((failures + 1))
  else
    echo "ok   - $desc (gate: fail, matched /$pattern/)"
  fi
}

sorry_gate() { "$here/check-sorry.sh" "$@"; }
axiom_gate() { "$here/check-axioms.sh" "$@"; }

# Positive control: the committed tree passes both gates.
d="$(fresh_copy)"
expect pass "unmodified tree passes sorry gate" sorry_gate "$d"
expect pass "unmodified tree passes axiom gate" axiom_gate "$d"

# Sorry gate: a sorry in a model theorem fails the build-log check and scan.
d="$(fresh_copy)"
cat >>"$d/Botho/Basic.lean" <<'EOF'

theorem Botho.selftest_sorry : 1 = 2 := by
  sorry
EOF
expect fail "sorry in Botho/Basic.lean" sorry_gate "$d"

# Sorry gate: the build-log check alone catches a sorry (via a sorryAx term),
# independent of the source scan.
d="$(fresh_copy)"
printf '\ntheorem Botho.selftest_sorryAx_term : 1 = 2 := sorryAx _ false\n' >>"$d/Botho/Basic.lean"
expect fail "sorryAx term in Botho/Basic.lean (build log only)" \
  sorry_gate --no-scan --no-sweep "$d"
expect_fail_with 'reaches sorryAx: Botho.selftest_sorryAx_term' \
  "sorryAx term in Botho/Basic.lean (sweep only)" \
  sorry_gate --no-scan "$d"
expect fail "sorryAx term in Botho/Basic.lean (scan only)" sorry_gate --no-build "$d"

# Sorry gate: `admit` is rejected by the source scan.
d="$(fresh_copy)"
cat >>"$d/Botho/Basic.lean" <<'EOF'

theorem Botho.selftest_admit : 1 = 2 := by
  admit
EOF
expect fail "admit in Botho/Basic.lean (scan only)" sorry_gate --no-build "$d"

# Sorry gate: a sorry in the theorem manifest is rejected by the scan.
d="$(fresh_copy)"
printf '\nexample : 1 = 2 := sorry\n' >>"$d/Theorems.lean"
expect fail "sorry in Theorems.lean (scan only)" sorry_gate --no-build "$d"
expect_fail_with 'only .import. lines' \
  "declaration in Theorems.lean (strict manifest)" axiom_gate "$d"

# Sorry gate: native_decide without the allow marker is rejected.
d="$(fresh_copy)"
printf '\ntheorem Botho.selftest_nd : 2 + 2 = 4 := by native_decide\n' >>"$d/Botho/Basic.lean"
expect fail "native_decide without allow marker" sorry_gate --no-build "$d"

# Negative controls: the token inside a comment, a string, or an allow-marked
# native_decide does not trip the scan.
d="$(fresh_copy)"
cat >>"$d/Botho/Basic.lean" <<'EOF'

-- a line comment that mentions sorry and admit
/- a block comment /- nested -/ that mentions sorry -/
def Botho.selftest_str : String := "sorry, admit"
theorem Botho.selftest_nd_ok : 2 + 2 = 4 := by native_decide -- formal-gate: allow native_decide
EOF
expect pass "sorry/admit in comments and strings, allow-marked native_decide" \
  sorry_gate --no-build "$d"

# Sorry gate (#1724): a char literal '"' must not open a phantom string that
# hides the code after it.
d="$(fresh_copy)"
cat >>"$d/Botho/Basic.lean" <<'EOF'

def Botho.selftest_quote : Char := '"'
theorem Botho.selftest_after_char : 1 = 2 := by
  sorry
EOF
expect fail "sorry after a '\"' char literal (scan only)" sorry_gate --no-build "$d"

# Sorry gate (#1724): a raw string containing `"` must not desynchronise the
# string tracking (one and two `#`).
d="$(fresh_copy)"
cat >>"$d/Botho/Basic.lean" <<'EOF'

def Botho.selftest_raw : String := r#"a"b"#
theorem Botho.selftest_after_raw : 1 = 2 := by
  sorry
EOF
expect fail "sorry after r#\"a\"b\"# raw string (scan only)" sorry_gate --no-build "$d"
d="$(fresh_copy)"
cat >>"$d/Botho/Basic.lean" <<'EOF'

def Botho.selftest_raw2 : String := r##"a"#b"##
theorem Botho.selftest_after_raw2 : 1 = 2 := by
  sorry
EOF
expect fail "sorry after r##\"a\"#b\"## raw string (scan only)" sorry_gate --no-build "$d"

# Sorry gate (#1724): the qualified `_root_.sorryAx` is caught like the bare name.
d="$(fresh_copy)"
printf '\ntheorem Botho.selftest_root_sorryAx : 1 = 2 := _root_.sorryAx _ false\n' \
  >>"$d/Botho/Basic.lean"
expect fail "_root_.sorryAx term (scan only)" sorry_gate --no-build "$d"

# Negative control (#1724): forbidden tokens inside raw strings and char
# literals, and primed identifiers next to them, do not trip the scan.
d="$(fresh_copy)"
cat >>"$d/Botho/Basic.lean" <<'EOF'

def Botho.selftest_raw_ok : String := r##"sorry "# admit"##
def Botho.selftest_raw0_ok : String := r"sorry \ admit"
def Botho.selftest_chars_ok : List Char := ['"', '\'', '\x41', 'α', 's']
theorem Botho.selftest_primed_ok' (h' : 1 = 1) : 1 = 1 := h'
EOF
expect pass "sorry/admit in raw strings, char literals and primed names" \
  sorry_gate --no-build "$d"
expect pass "raw-string/char-literal control tree still builds" sorry_gate --no-scan "$d"

# Axiom gate (#1724): theorem names ending in `'` parse (one and two primes).
d="$(fresh_copy)"
cat >>"$d/Botho/Basic.lean" <<'EOF'

theorem Botho.selftest_primed' : 1 + 1 = 2 := rfl
theorem Botho.selftest_primed'' : ∀ p : Prop, p ∨ ¬p := Classical.em
EOF
cat >>"$d/Theorems.lean" <<'EOF'
#print axioms Botho.selftest_primed'
#print axioms Botho.selftest_primed''
EOF
expect pass "primed theorem names parse in the axiom gate" axiom_gate "$d"

# Axiom gate (#1724): a primed theorem that reaches sorryAx still fails.
d="$(fresh_copy)"
cat >>"$d/Botho/Basic.lean" <<'EOF'

theorem Botho.selftest_primed_sorry' : 1 = 2 := sorry
EOF
printf '#print axioms %s\n' "Botho.selftest_primed_sorry'" >>"$d/Theorems.lean"
expect fail "primed theorem depending on sorryAx" axiom_gate "$d"

# Axiom gate: a bogus axiom used by a manifest theorem fails.
d="$(fresh_copy)"
cat >>"$d/Botho/Basic.lean" <<'EOF'

axiom Botho.selftestBogus : 1 = 2
theorem Botho.selftest_uses_bogus : 1 = 2 := Botho.selftestBogus
EOF
printf '#print axioms Botho.selftest_uses_bogus\n' >>"$d/Theorems.lean"
expect fail "theorem depending on a non-allow-listed axiom" axiom_gate "$d"

# Axiom gate: a manifest theorem that reaches sorryAx fails.
d="$(fresh_copy)"
cat >>"$d/Botho/Basic.lean" <<'EOF'

theorem Botho.selftest_sorryAx : 1 = 2 := sorry
EOF
printf '#print axioms %s\n' "Botho.selftest_sorryAx" >>"$d/Theorems.lean"
expect fail "theorem depending on sorryAx" axiom_gate "$d"

# Axiom gate: an empty manifest fails.
d="$(fresh_copy)"
printf 'import Botho\n' >"$d/Theorems.lean"
expect fail "empty theorem manifest" axiom_gate "$d"

# Axiom gate: a manifest that does not compile fails.
d="$(fresh_copy)"
printf '\n#print axioms Botho.no_such_theorem\n' >>"$d/Theorems.lean"
expect fail "manifest naming a missing theorem" axiom_gate "$d"

# ---- #1770: bypasses found by the Judge of PR #1749, and others ----

# A theorem that depends on a non-allow-listed axiom, used by the spoof cases.
bogus_thm() {
  cat >>"$1/Botho/Basic.lean" <<'EOF'

axiom Botho.selftestBogus : 1 = 2
theorem Botho.selftest_spoofed : 1 = 2 := Botho.selftestBogus
EOF
}

# Axiom gate (#1770): a fake result line printed by `#eval` after the real
# `#print axioms` must not override the real result.
d="$(fresh_copy)"
bogus_thm "$d"
cat >>"$d/Theorems.lean" <<'EOF'
#print axioms Botho.selftest_spoofed
#eval IO.println "'Botho.selftest_spoofed' does not depend on any axioms"
EOF
expect_fail_with 'only .import. lines' \
  "#eval-printed fake '#print axioms' result" axiom_gate "$d"

# Axiom gate (#1770): a `#print axioms` inside a block comment is not a
# command, and a fake line cannot make up the count.
d="$(fresh_copy)"
bogus_thm "$d"
cat >>"$d/Theorems.lean" <<'EOF'
/- #print axioms Botho.selftest_spoofed -/
#eval IO.println "'Botho.selftest_spoofed' does not depend on any axioms"
EOF
expect_fail_with 'only .import. lines' \
  "commented-out '#print axioms' plus a fake result" axiom_gate "$d"

# Axiom gate (#1770): output from an imported module's initializer (which
# runs when Theorems.lean imports Botho) is not a Lean message and is
# rejected, even with a plain manifest and a clean theorem.
d="$(fresh_copy)"
cat >>"$d/Botho/Basic.lean" <<'EOF'

initialize IO.println "'Botho.transfer_conserves' does not depend on any axioms"
EOF
expect_fail_with 'not a Lean message' \
  "imported initializer printing a fake result" axiom_gate "$d"
expect_fail_with "forbidden token 'initialize'" \
  "initialize in a model source (scan only)" sorry_gate --no-build "$d"

# Axiom gate (#1770): an initializer that prints a well-formed JSON message at
# the position of a real `#print axioms` command gives that command a second
# result, which is rejected.
d="$(fresh_copy)"
bogus_thm "$d"
line=$(($(wc -l <"$d/Theorems.lean") + 1))
spoof="$(cat <<'EOF'
initialize IO.println "{\"pos\":{\"line\":@LINE@,\"column\":0},\"severity\":\"information\",\"data\":\"'Botho.selftest_spoofed' does not depend on any axioms\"}"
EOF
)"
printf '\n%s\n' "${spoof/@LINE@/$line}" >>"$d/Botho/Basic.lean"
printf '#print axioms Botho.selftest_spoofed\n' >>"$d/Theorems.lean"
expect_fail_with 'second result for Botho.selftest_spoofed' \
  "initializer printing a fake JSON result at a command position" axiom_gate "$d"

# Axiom gate (#1770): a model that redefines `#print axioms` with
# macro_rules gets one fake result at each real command position. The
# independent walk (AxiomReport.lean) disagrees, and the gate fails.
d="$(fresh_copy)"
bogus_thm "$d"
cat >>"$d/Botho/Basic.lean" <<'EOF'

macro_rules
  | `(#print axioms $id:ident) =>
    let s := Lean.Syntax.mkStrLit s!"'{id.getId}' does not depend on any axioms"
    `(#eval IO.println $s)
EOF
printf '#print axioms Botho.selftest_spoofed\n' >>"$d/Theorems.lean"
expect_fail_with 'is .#print axioms. overridden' \
  "'#print axioms' overridden by macro_rules" axiom_gate "$d"
expect_fail_with "forbidden token '#eval'" \
  "#eval in a model source (scan only)" sorry_gate --no-build "$d"

# Axiom gate (#1770): a declaration admitted with kernel checking switched
# off (debug.skipKernelTC) is caught by the leanchecker replay, even though
# `#print axioms` reports no axioms for it.
d="$(fresh_copy)"
{ printf 'import Lean\n'; cat "$d/Botho/Basic.lean"; } >"$d/Basic.tmp"
mv "$d/Basic.tmp" "$d/Botho/Basic.lean"
cat >>"$d/Botho/Basic.lean" <<'EOF'

open Lean Elab Command in
set_option debug.skipKernelTC true in
run_cmd liftCoreM <| addDecl <| .thmDecl {
  name := `Botho.selftest_unchecked, levelParams := [],
  type := mkApp3 (mkConst ``Eq [1]) (mkConst ``Nat) (mkNatLit 1) (mkNatLit 2),
  value := mkApp2 (mkConst ``Eq.refl [1]) (mkConst ``Nat) (mkNatLit 1) }
EOF
printf '#print axioms Botho.selftest_unchecked\n' >>"$d/Theorems.lean"
expect_fail_with 'leanchecker rejected' \
  "theorem admitted with debug.skipKernelTC (kernel replay)" axiom_gate "$d"
expect_fail_with "forbidden token 'skipKernelTC'" \
  "run_cmd / debug.skipKernelTC in a model source (scan only)" \
  sorry_gate --no-build "$d"

# Sorry and axiom gates (#1770): `#guard_msgs` swallows the "declaration uses
# sorry" warning of a `stop` proof, so the build log is clean. The sweep, the
# scan and the axiom gate each still catch it, listed in the manifest or not.
d="$(fresh_copy)"
cat >>"$d/Botho/Basic.lean" <<'EOF'

/-- warning: declaration uses `sorry` -/
#guard_msgs in
theorem Botho.selftest_hidden : 1 = 2 := by
  stop
  simp
EOF
expect_fail_with 'reaches sorryAx: Botho.selftest_hidden' \
  "#guard_msgs-hidden stop proof (sweep only)" sorry_gate --no-scan "$d"
expect_fail_with "forbidden token '#guard_msgs'" \
  "#guard_msgs in a model source (scan only)" sorry_gate --no-build "$d"
expect_fail_with 'Botho.selftest_hidden reaches sorryAx' \
  "#guard_msgs-hidden stop proof, unlisted (axiom gate sweep)" axiom_gate "$d"
printf '#print axioms Botho.selftest_hidden\n' >>"$d/Theorems.lean"
expect_fail_with 'Botho.selftest_hidden depends on sorryAx' \
  "#guard_msgs-hidden stop proof, listed (axiom gate)" axiom_gate "$d"

# Sorry gate (#1770): `sorryAx.{0}` (explicit universe) is caught by the scan.
d="$(fresh_copy)"
printf '\ntheorem Botho.selftest_univ : 1 = 2 := sorryAx.{0} (1 = 2) false\n' \
  >>"$d/Botho/Basic.lean"
expect_fail_with "forbidden token 'sorryAx'" \
  "sorryAx.{0} term (scan only)" sorry_gate --no-build "$d"

# Sorry gate (#1770): code inside `{...}` string interpolation is scanned,
# including after a nested string literal.
d="$(fresh_copy)"
printf '\ndef Botho.selftest_interp : String := s!"{(sorry : Nat)}"\n' \
  >>"$d/Botho/Basic.lean"
expect_fail_with "forbidden token 'sorry'" \
  "sorry in s!\"{...}\" interpolation (scan only)" sorry_gate --no-build "$d"
d="$(fresh_copy)"
cat >>"$d/Botho/Basic.lean" <<'EOF'

def Botho.selftest_interp2 : String := s!"{String.append "a" "b"} {(sorry : Nat)}"
EOF
expect_fail_with "forbidden token 'sorry'" \
  "sorry after a nested string in interpolation (scan only)" \
  sorry_gate --no-build "$d"

# Negative control (#1770): interpolation without forbidden code, and the
# word "sorry" in the literal part of an interpolated string, pass.
d="$(fresh_copy)"
cat >>"$d/Botho/Basic.lean" <<'EOF'

def Botho.selftest_interp_ok (n : Nat) : String :=
  s!"sorry? n = {n + 1}, {String.append "a" "b"}"
EOF
expect pass "interpolation without forbidden code" sorry_gate --no-build "$d"
expect pass "interpolation control tree still builds" sorry_gate --no-scan "$d"

# Axiom gate (#1770): duplicate `#print axioms` lines are rejected.
d="$(fresh_copy)"
printf '#print axioms Botho.transfer_conserves\n' >>"$d/Theorems.lean"
expect_fail_with 'already listed' "duplicate '#print axioms' line" axiom_gate "$d"

# Axiom gate (#1770): the manifest may import only Botho modules.
d="$(fresh_copy)"
{ printf 'import Lean\n'; cat "$d/Theorems.lean"; } >"$d/Theorems.tmp"
mv "$d/Theorems.tmp" "$d/Theorems.lean"
expect_fail_with 'imports outside the Botho library' \
  "manifest importing a non-Botho module" axiom_gate "$d"

# Both gates (#1770): a lakefile.lean (a Lean program run by every lake call)
# or a lakefile.toml that passes extra arguments to lean is rejected.
d="$(fresh_copy)"
printf 'import Lake\nopen Lake DSL\npackage botho_formal\n@[default_target] lean_lib Botho\n' \
  >"$d/lakefile.lean"
expect_fail_with 'lakefile.lean is not allowed' \
  "lakefile.lean present (axiom gate)" axiom_gate "$d"
expect_fail_with 'lakefile.lean is not allowed' \
  "lakefile.lean present (sorry gate)" sorry_gate --no-scan "$d"
d="$(fresh_copy)"
printf '\nmoreLeanArgs = ["-Ddebug.skipKernelTC=true"]\n' >>"$d/lakefile.toml"
expect_fail_with 'forbidden build option' \
  "lakefile.toml with moreLeanArgs" axiom_gate "$d"

echo
if [[ "$failures" != 0 ]]; then
  echo "selftest: $failures case(s) FAILED"
  exit 1
fi
echo "selftest: all cases behaved as designed"
