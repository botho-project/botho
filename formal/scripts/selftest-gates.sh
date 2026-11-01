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
# The axiom gate runs the source scan first (#1770 second review). Cases that
# test a later layer on its own skip the scan with --no-scan, and where it
# matters a second run proves the full gate fails too.

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
  "declaration in Theorems.lean (strict manifest)" axiom_gate --no-scan "$d"

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
expect_fail_with "selftest_primed_sorry' depends on sorryAx" \
  "primed theorem depending on sorryAx (no scan)" axiom_gate --no-scan "$d"
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
expect_fail_with 'selftest_sorryAx depends on sorryAx' \
  "theorem depending on sorryAx (no scan)" axiom_gate --no-scan "$d"
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
  "#eval-printed fake '#print axioms' result" axiom_gate --no-scan "$d"

# Axiom gate (#1770): a `#print axioms` inside a block comment is not a
# command, and a fake line cannot make up the count.
d="$(fresh_copy)"
bogus_thm "$d"
cat >>"$d/Theorems.lean" <<'EOF'
/- #print axioms Botho.selftest_spoofed -/
#eval IO.println "'Botho.selftest_spoofed' does not depend on any axioms"
EOF
expect_fail_with 'only .import. lines' \
  "commented-out '#print axioms' plus a fake result" axiom_gate --no-scan "$d"

# Axiom gate (#1770): output from an imported module's initializer (which
# runs when Theorems.lean imports Botho) is not a Lean message and is
# rejected, even with a plain manifest and a clean theorem.
d="$(fresh_copy)"
cat >>"$d/Botho/Basic.lean" <<'EOF'

initialize IO.println "'Botho.transfer_conserves' does not depend on any axioms"
EOF
expect_fail_with 'not a Lean message' \
  "imported initializer printing a fake result" axiom_gate --no-scan "$d"
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
  "initializer printing a fake JSON result at a command position" \
  axiom_gate --no-scan "$d"

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
  "'#print axioms' overridden by macro_rules" axiom_gate --no-scan "$d"
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
  "theorem admitted with debug.skipKernelTC (kernel replay)" \
  axiom_gate --no-scan "$d"
expect_fail_with "import of 'Lean' is not allowed" \
  "import Lean / run_cmd in a model source (full axiom gate)" axiom_gate "$d"
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
  "#guard_msgs-hidden stop proof, unlisted (axiom gate sweep)" \
  axiom_gate --no-scan "$d"
printf '#print axioms Botho.selftest_hidden\n' >>"$d/Theorems.lean"
expect_fail_with 'Botho.selftest_hidden depends on sorryAx' \
  "#guard_msgs-hidden stop proof, listed (axiom gate)" axiom_gate --no-scan "$d"
expect_fail_with "forbidden token '#guard_msgs'" \
  "#guard_msgs-hidden stop proof (full axiom gate)" axiom_gate "$d"

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
  "manifest importing a non-Botho module (no scan)" axiom_gate --no-scan "$d"
expect_fail_with "Theorems.lean:1: import of 'Lean' is not allowed" \
  "manifest importing a non-Botho module (full axiom gate)" axiom_gate "$d"

# Both gates (#1770): a lakefile.lean (a Lean program run by every lake call)
# is rejected.
d="$(fresh_copy)"
printf 'import Lake\nopen Lake DSL\npackage botho_formal\n@[default_target] lean_lib Botho\n' \
  >"$d/lakefile.lean"
expect_fail_with 'lakefile.lean is not allowed' \
  "lakefile.lean present (axiom gate)" axiom_gate "$d"
expect_fail_with 'lakefile.lean is not allowed' \
  "lakefile.lean present (sorry gate)" sorry_gate --no-scan "$d"

# ---- #1770 review: code that runs during `lake build` ----

# Axiom gate (#1770 review): the Judge's bypass. An `@[init]` initializer
# (no `import Lean`, no banned keyword) runs during `lake build` and appends
# the cheat axiom to axioms-allowlist.txt before the gate reads it. The gate
# reads the allow-list it snapshotted before the build, so it still rejects
# the cheat axiom. The second check proves the initializer really ran.
init_widen() {
  cat >>"$1/Botho/Basic.lean" <<'EOF'

namespace Botho
axiom cheat : False
theorem one_eq_two : 1 = 2 := cheat.elim
def widen : IO Unit := do
  let p := "axioms-allowlist.txt"
  let s ← IO.FS.readFile p
  unless (s.splitOn "Botho.cheat").length > 1 do
    IO.FS.writeFile p (s ++ "Botho.cheat\n")
@[init widen] opaque widenInit : Unit
end Botho
EOF
  printf '#print axioms Botho.one_eq_two\n' >>"$1/Theorems.lean"
}
d="$(fresh_copy)"
init_widen "$d"
expect_fail_with 'one_eq_two depends on non-allow-listed axiom Botho.cheat' \
  "@[init] initializer widening the allow-list during the build (no scan)" \
  axiom_gate --no-scan "$d"
expect pass "  (the initializer did rewrite axioms-allowlist.txt)" \
  grep -q '^Botho.cheat$' "$d/axioms-allowlist.txt"
expect_fail_with "forbidden attribute 'init'" \
  "@[init f] in a model source (scan only)" sorry_gate --no-build "$d"
d="$(fresh_copy)"
init_widen "$d"
expect_fail_with "forbidden attribute 'init'" \
  "@[init] initializer widening the allow-list (full axiom gate)" axiom_gate "$d"
expect fail "  (the failed scan stopped the gate before the build ran it)" \
  grep -q '^Botho.cheat$' "$d/axioms-allowlist.txt"

# Sorry gate (#1770 review): an initializer that deletes its own `@[init]`
# line from the source during the build. The scan runs before the build, so
# it still sees the attribute, and a failed scan stops the gate before the
# build runs the initializer at all. With --no-scan the build does run it.
d="$(fresh_copy)"
cat >>"$d/Botho/Basic.lean" <<'EOF'

namespace Botho
def hide : IO Unit := do
  let p := "Botho/Basic.lean"
  let s ← IO.FS.readFile p
  IO.FS.writeFile p (s.replace "@[init hide]" "")
@[init hide] opaque hideInit : Unit
end Botho
EOF
expect_fail_with "forbidden attribute 'init'" \
  "self-erasing @[init] initializer (full sorry gate)" sorry_gate "$d"
expect pass "  (the failed scan stopped the gate before the build)" \
  grep -q '@\[init hide\]' "$d/Botho/Basic.lean"
expect pass "  (without the scan the build runs the initializer)" \
  sorry_gate --no-scan "$d"
expect fail "  (and the initializer did erase its @[init] line)" \
  grep -q '@\[init hide\]' "$d/Botho/Basic.lean"

# Sorry gate (#1770 review): initializer and other code-running attributes in
# every attribute position: lists, `attribute [...]`, `local` / `scoped`,
# entries split over lines, guillemets, and the attribute-erasing `-` form.
d="$(fresh_copy)"
cat >>"$d/Botho/Basic.lean" <<'EOF'

def Botho.selftest_io : IO Unit := pure ()
@[simp, init Botho.selftest_io] opaque Botho.selftest_w1 : Unit
EOF
expect_fail_with "forbidden attribute 'init'" \
  "@[simp, init f] attribute list (scan only)" sorry_gate --no-build "$d"
for case in \
  'init|attribute [init Botho.selftest_io] Botho.selftest_w' \
  'init|attribute [simp, local init Botho.selftest_io] Botho.selftest_w' \
  'init|@[scoped init Botho.selftest_io] opaque Botho.selftest_w : Unit' \
  'init|@[«init» Botho.selftest_io] opaque Botho.selftest_w : Unit' \
  'builtin_init|@[builtin_init Botho.selftest_io] opaque Botho.selftest_w : Unit' \
  'extern|@[extern "system"] opaque Botho.selftest_w : Nat → Nat' \
  'implemented_by|@[implemented_by Botho.selftest_io] opaque Botho.selftest_w : IO Unit' \
  'export|@[export botho_w] def Botho.selftest_w : Nat := 0' \
  'macro|@[macro Botho.selftest_k] def Botho.selftest_w : Nat := 0' \
  'command_elab|@[command_elab Botho.selftest_k] def Botho.selftest_w : Nat := 0' \
  'term_elab|@[term_elab Botho.selftest_k] def Botho.selftest_w : Nat := 0'; do
  d="$(fresh_copy)"
  printf '\n%s\n' "${case#*|}" >>"$d/Botho/Basic.lean"
  expect_fail_with "forbidden attribute '${case%%|*}'" \
    "attribute: ${case#*|} (scan only)" sorry_gate --no-build "$d"
done
d="$(fresh_copy)"
cat >>"$d/Botho/Basic.lean" <<'EOF'

@[simp
  , init Botho.selftest_io] opaque Botho.selftest_w2 : Unit
EOF
expect_fail_with "Basic.lean:[0-9]+: forbidden attribute 'init'" \
  "init on the second line of an attribute list (scan only)" \
  sorry_gate --no-build "$d"

# Sorry gate (#1770 review): metaprogramming commands, which can expand to a
# banned command built from raw syntax, are rejected by the scan.
for cmd in \
  'macro "botho_m" : term => `(1)' \
  'syntax "botho_s" : term' \
  'elab "botho_e" : term => pure (Lean.mkNatLit 1)' \
  'declare_syntax_cat botho_cat'; do
  d="$(fresh_copy)"
  printf '\n%s\n' "$cmd" >>"$d/Botho/Basic.lean"
  expect_fail_with "forbidden token '${cmd%% *}'" \
    "command: ${cmd%% *} (scan only)" sorry_gate --no-build "$d"
done

# Negative control (#1770 review): `init` as a variable name, harmless
# attributes, and `init` in a comment or string pass the scan and build.
d="$(fresh_copy)"
cat >>"$d/Botho/Basic.lean" <<'EOF'

def Botho.selftest_init_ok (init : Nat) : Nat := init + 1
@[simp] theorem Botho.selftest_init_simp (init : Nat) :
    Botho.selftest_init_ok init = init + 1 := rfl
attribute [local simp] Botho.selftest_init_ok
-- @[init foo] in a comment
def Botho.selftest_init_str : String := "@[init foo]"
EOF
expect pass "init as a variable, @[simp], attribute [local simp] (scan only)" \
  sorry_gate --no-build "$d"
expect pass "init control tree still builds (full sorry gate)" sorry_gate "$d"

# ---- #1770 second review: imports and the build layout ----

# Both gates (#1770 second review): the Judge's simproc bypass. A simproc (a
# compile-time `MetaM` hook, which needs `import Lean`) runs during
# `lake build` and overwrites the compiled Botho/Cheat.olean with a forged
# one in which `one_eq_two` has no axioms. leanchecker, the walk and
# `#print axioms` all read the forged file, so only the import allow-list in
# the source scan stops it. The control run with --no-scan proves the swap
# really fools the later layers, so the scan cases are not vacuous.
simproc_swap() {
  cat >"$1/Botho/Cheat.lean" <<'EOF'
namespace Botho
axiom cheat : False
theorem one_eq_two : 1 = 2 := cheat.elim
end Botho
EOF
  cat >"$1/Botho/Evil.lean" <<'EOF'
import Lean
import Botho.Cheat
open Lean Meta Simp in
simproc botho_tidy (Nat.succ _) := fun _ => do
  let cwd ← IO.currentDir
  let dir := cwd / ".lake" / "tidy"
  IO.FS.createDirAll dir
  IO.FS.writeFile (dir / "Cheat.lean") "namespace Botho\ntheorem one_eq_two : True := trivial\nend Botho\n"
  let exe := (← IO.appPath).toString
  let args := #["-o", (cwd / ".lake/build/lib/lean/Botho/Cheat.olean").toString, (dir / "Cheat.lean").toString]
  let _ ← IO.Process.output { cmd := exe, args := args }
  return .continue

example : Nat.succ 1 = 2 := by simp only [botho_tidy]
EOF
  printf 'import Botho.Basic\nimport Botho.Cheat\nimport Botho.Evil\n' >"$1/Botho.lean"
  printf '#print axioms Botho.one_eq_two\n' >>"$1/Theorems.lean"
}
d="$(fresh_copy)"
simproc_swap "$d"
expect_fail_with "Evil.lean:1: import of 'Lean' is not allowed" \
  "simproc overwriting a compiled .olean (full sorry gate)" sorry_gate "$d"
expect_fail_with "Evil.lean:1: import of 'Lean' is not allowed" \
  "simproc overwriting a compiled .olean (full axiom gate)" axiom_gate "$d"
expect pass "  (control: without the scan the forged .olean fools the axiom gate)" \
  axiom_gate --no-scan "$d"

# Both gates (#1770 second review): a bare `import Lean` in a model module.
d="$(fresh_copy)"
{ printf 'import Lean\n'; cat "$d/Botho/Basic.lean"; } >"$d/Basic.tmp"
mv "$d/Basic.tmp" "$d/Botho/Basic.lean"
expect_fail_with "Basic.lean:1: import of 'Lean' is not allowed" \
  "bare import Lean in Botho/Basic.lean (scan only)" sorry_gate --no-build "$d"
expect_fail_with "Basic.lean:1: import of 'Lean' is not allowed" \
  "bare import Lean in Botho/Basic.lean (full axiom gate)" axiom_gate "$d"

# Sorry gate (#1770 second review): the import allow-list in every header
# form: other packages, `Init`, guillemets, imports split over lines with
# comments between them, header keywords and modifiers, a module that is not
# a scanned file, and an `import` after the header. Each replaces Botho.lean.
for case in \
  "import of .Std. is not allowed|import Botho.Basic\nimport Std" \
  "import of .Lake. is not allowed|import Lake\nimport Botho.Basic" \
  "import of .Mathlib. is not allowed|import Mathlib\nimport Botho.Basic" \
  "import of .Init.Data.List. is not allowed|import Init.Data.List\nimport Botho.Basic" \
  "import of .Lean.Elab.Command. is not allowed|import Botho.Basic\nimport Lean.Elab.Command" \
  "import of .Lean. is not allowed|import Botho.Basic\nimport «Lean»" \
  "import of .Lean. is not allowed|/- doc -/\nimport Botho.Basic\nimport\tLean" \
  "Botho.lean:5: import of .Lean. is not allowed|import Botho.Basic\n/- c -/ import -- c\n  /- nested /- c -/ -/\n\n  Lean" \
  "header keyword .prelude.|prelude\nimport Botho.Basic" \
  "header keyword .module.|module\nimport Botho.Basic" \
  "import modifier .public.|public import Botho.Basic" \
  "import modifier .meta.|meta import Botho.Basic" \
  "import of .all. is not allowed|import all Botho.Basic" \
  "does not resolve to a scanned file|import Botho.Basic\nimport Botho.Missing" \
  "malformed import|import Botho.«Ba sic»" \
  ".import. after the module header|import Botho.Basic\ndef Botho.x : Nat := 1\nimport Lean"; do
  d="$(fresh_copy)"
  printf '%b\n' "${case#*|}" >"$d/Botho.lean"
  expect_fail_with "${case%%|*}" \
    "Botho.lean header: $(printf '%b' "${case#*|}" | tr '\n\t' '  ') (scan only)" \
    sorry_gate --no-build "$d"
done

# Sorry gate (#1770 second review): the simproc commands are also banned
# tokens, as a second layer behind the import allow-list.
d="$(fresh_copy)"
printf '\nsimproc botho_s (Nat.succ _) := fun _ => pure .continue\n' >>"$d/Botho/Basic.lean"
expect_fail_with "forbidden token 'simproc'" "simproc command (scan only)" \
  sorry_gate --no-build "$d"

# Both gates (#1770 second review): the layout. Model sources must be regular
# files in Botho.lean and Botho/, and no other .lean file may exist.
d="$(fresh_copy)"
printf 'def Botho.outside : Nat := 1\n' >"$scratch/outside.lean"
ln -s "$scratch/outside.lean" "$d/Botho/Link.lean"
expect_fail_with 'symlink under Botho/' "symlinked Botho/Link.lean (scan only)" \
  sorry_gate --no-build "$d"
d="$(fresh_copy)"
mv "$d/Botho" "$d/Botho.real"
ln -s "$d/Botho.real" "$d/Botho"
expect_fail_with 'must be a directory' "Botho/ as a symlink (full axiom gate)" \
  axiom_gate "$d"
d="$(fresh_copy)"
printf 'def Botho.stray : Nat := 1\n' >"$d/Extra.lean"
expect_fail_with 'Extra.lean: Lean file outside the scanned set' \
  "stray Extra.lean next to Botho.lean (full axiom gate)" axiom_gate "$d"

# Both gates (#1770 second review): the Judge's srcDir bypass. The sources
# move to src/ and lakefile.toml points Lake there, so a scan of the fixed
# paths would see none of them. The layout check rejects it.
d="$(fresh_copy)"
mkdir "$d/src"
mv "$d/Botho.lean" "$d/Botho" "$d/src/"
printf 'srcDir = "src"\n' >>"$d/lakefile.toml"
printf '\nnamespace Botho\ndef pwn : IO Unit := IO.FS.writeFile "PWNED" ""\n@[init pwn] opaque pwnInit : Unit\nend Botho\n' \
  >>"$d/src/Botho/Basic.lean"
expect_fail_with 'lakefile.toml changes the build layout' \
  "srcDir = \"src\" (full sorry gate)" sorry_gate "$d"
expect_fail_with 'lakefile.toml changes the build layout' \
  "srcDir = \"src\" (full axiom gate)" axiom_gate "$d"
expect fail "  (the initializer never ran)" test -e "$d/PWNED"

# Both gates (#1770 second review): other layout changes are rejected. Since
# #1799 only the layout keys are checked (the same keys as the verify job's
# layout check): the old exact-text pins on lakefile.toml, lean-toolchain and
# lake-manifest.json are gone, because the verify job runs this scan from the
# base branch and accepts other lakefile edits and toolchain bumps.
for case in \
  'extra lean_lib|\n[[lean_lib]]\nname = "Evil"' \
  'lean_exe|\n[[lean_exe]]\nname = "evil"\nroot = "Evil"' \
  'globs|globs = ["Botho.+", "Evil"]' \
  'roots|roots = ["Botho", "Evil"]' \
  'lean_lib srcDir|srcDir = ".lake/evil"'; do
  d="$(fresh_copy)"
  printf '%b\n' "${case#*|}" >>"$d/lakefile.toml"
  expect_fail_with 'lakefile.toml changes the build layout' \
    "lakefile.toml with ${case%%|*} (sorry gate, no scan)" sorry_gate --no-scan "$d"
  expect_fail_with 'lakefile.toml changes the build layout' \
    "lakefile.toml with ${case%%|*} (axiom gate)" axiom_gate "$d"
done
d="$(fresh_copy)"
printf 'name = [\n' >>"$d/lakefile.toml"
expect_fail_with 'cannot parse lakefile.toml' \
  "unparsable lakefile.toml (sorry gate, no scan)" sorry_gate --no-scan "$d"
# Accepted: a lakefile edit that keeps the layout (the verify job's
# lakefile-benign fixture: a version bump, a comment and leanOptions).
d="$(fresh_copy)"
cp "$here/fixtures/verify/lakefile-benign/lakefile.toml" "$d/lakefile.toml"
expect pass "benign lakefile edit, same layout (full sorry gate)" sorry_gate "$d"
expect pass "benign lakefile edit, same layout (full axiom gate)" axiom_gate "$d"
# The verify job runs the base branch's check-sorry.sh like this: scan only,
# with LAKE=false so a `lake` call would fail. It must accept the clean tree.
d="$(fresh_copy)"
rm -rf "$d/.lake"
expect pass "verify-job mode: --no-build with LAKE=false (clean tree)" \
  env LAKE=false "$here/check-sorry.sh" --no-build "$d"
expect fail "  (and nothing was built)" test -e "$d/.lake"

# Negative control (#1770 second review): a second module imported through
# guillemets and a multi-line import with comments between the imports,
# `private def` right after the header, "import Lean" in a comment and a
# string, and a lakefile.toml with comments, blank lines and indentation.
# Both full gates pass.
d="$(fresh_copy)"
mkdir -p "$d/Botho/Extra"
cat >"$d/Botho.lean" <<'EOF'
/- Root import (self-test control). import Lean in a comment. -/
import Botho.Basic -- the toy model
/- a comment between imports: import Lean -/
import
  «Botho».Extra.«Deep»
EOF
cat >"$d/Botho/Extra/Deep.lean" <<'EOF'
import Botho.Basic
-- import Lean (in a comment)
private def Botho.selftest_deep_helper : Nat := 1
def Botho.selftest_deep_str : String := "import Lean"
theorem Botho.selftest_deep : Botho.selftest_deep_helper = 1 := rfl
EOF
printf '#print axioms Botho.selftest_deep\n' >>"$d/Theorems.lean"
cat >"$d/lakefile.toml" <<'EOF'
# Botho formal model: one library, no dependencies.
name = "botho_formal"
  version = "0.1.0"

defaultTargets = ["Botho"]

[[lean_lib]]
name = "Botho"   
EOF
expect pass "guillemet and multi-line imports, commented lakefile (full sorry gate)" \
  sorry_gate "$d"
expect pass "guillemet and multi-line imports, commented lakefile (full axiom gate)" \
  axiom_gate "$d"

echo
if [[ "$failures" != 0 ]]; then
  echo "selftest: $failures case(s) FAILED"
  exit 1
fi
echo "selftest: all cases behaved as designed"
