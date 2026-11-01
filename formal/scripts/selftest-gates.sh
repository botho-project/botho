#!/usr/bin/env bash
# Self-test for the formal gates (#1705).
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
expect fail "sorryAx term in Botho/Basic.lean (build log only)" sorry_gate --no-scan "$d"
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
cat >>"$d/Theorems.lean" <<'EOF'

theorem Botho.selftest_primed' : 1 + 1 = 2 := rfl
theorem Botho.selftest_primed'' : ∀ p : Prop, p ∨ ¬p := Classical.em
#print axioms Botho.selftest_primed'
#print axioms Botho.selftest_primed''
EOF
expect pass "primed theorem names parse in the axiom gate" axiom_gate "$d"

# Axiom gate (#1724): a primed theorem that reaches sorryAx still fails.
d="$(fresh_copy)"
cat >>"$d/Theorems.lean" <<'EOF'

theorem Botho.selftest_primed_sorry' : 1 = 2 := sorry
#print axioms Botho.selftest_primed_sorry'
EOF
expect fail "primed theorem depending on sorryAx" axiom_gate "$d"

# Axiom gate: a bogus axiom used by a manifest theorem fails.
d="$(fresh_copy)"
cat >>"$d/Theorems.lean" <<'EOF'

axiom Botho.selftestBogus : 1 = 2
theorem Botho.selftest_uses_bogus : 1 = 2 := Botho.selftestBogus
#print axioms Botho.selftest_uses_bogus
EOF
expect fail "theorem depending on a non-allow-listed axiom" axiom_gate "$d"

# Axiom gate: a manifest theorem that reaches sorryAx fails.
d="$(fresh_copy)"
cat >>"$d/Theorems.lean" <<'EOF'

theorem Botho.selftest_sorryAx : 1 = 2 := sorry
#print axioms Botho.selftest_sorryAx
EOF
expect fail "theorem depending on sorryAx" axiom_gate "$d"

# Axiom gate: an empty manifest fails.
d="$(fresh_copy)"
printf 'import Botho\n' >"$d/Theorems.lean"
expect fail "empty theorem manifest" axiom_gate "$d"

# Axiom gate: a manifest that does not compile fails.
d="$(fresh_copy)"
printf '\n#print axioms Botho.no_such_theorem\n' >>"$d/Theorems.lean"
expect fail "manifest naming a missing theorem" axiom_gate "$d"

echo
if [[ "$failures" != 0 ]]; then
  echo "selftest: $failures case(s) FAILED"
  exit 1
fi
echo "selftest: all cases behaved as designed"
