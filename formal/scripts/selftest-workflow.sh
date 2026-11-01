#!/usr/bin/env bash
# Self-test for the verify job's verifier selection (#1804).
#
# Extracts the `run:` block of the step named STEP from
# .github/workflows/formal.yml and runs it the way GitHub Actions runs a
# `bash` step (`bash --noprofile --norc -eo pipefail`), in a scratch
# directory laid out like the verify job's workspace, with stub verifiers in
# place of verify-artifacts.sh. It asserts that:
#
#   1. with no trusted/formal/scripts/verify-artifacts.sh (the base commit
#      has no verifier), the step fails, and the pull request's copy under
#      untrusted/ does not run, even though that stub would pass;
#   2. with a trusted verifier that passes, the step passes and runs the
#      trusted copy, with the base allow-list and base lakefile;
#   3. with a trusted verifier that fails, the step fails.
#
# Needs only bash, awk and coreutils: no Lean toolchain.
set -euo pipefail

STEP='Verify (base-commit gates over the compiled library)'

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
workflow="$(cd "$here/../.." && pwd)/.github/workflows/formal.yml"

# Print the dedented body of the step's `run: |` block.
extract_run() {
  awk -v step="- name: $STEP" '
    function indent(s) { match(s, /^ */); return RLENGTH }
    !found { t = $0; sub(/^ +/, "", t); if (t == step) { found = 1; si = indent($0) } ; next }
    found && !inrun {
      if ($0 ~ /^ *- / && indent($0) <= si) exit
      if ($0 ~ /^ *run: *\|-? *$/) { inrun = 1; ri = indent($0) }
      next
    }
    inrun {
      if ($0 ~ /^ *$/) { print ""; next }
      if (indent($0) <= ri) exit
      if (!bi) bi = indent($0)
      print substr($0, bi + 1)
    }
  ' "$workflow"
}

script="$(extract_run)"
if [[ -z "$script" ]]; then
  echo "SELFTEST FAIL: no run block for step '$STEP' in $workflow" >&2
  exit 1
fi
# shellcheck disable=SC2016  # a literal ${{, not an expansion
if grep -qF '${{' <<<"$script"; then
  echo "SELFTEST FAIL: the run block uses \${{ }} expressions; this test cannot run it" >&2
  exit 1
fi

tmp="$(mktemp -d)"
# shellcheck disable=SC2064  # expand now
trap "rm -rf '$tmp'" EXIT
printf '%s\n' "$script" >"$tmp/step.sh"

# stub DIR LABEL STATUS: a verify-artifacts.sh that records LABEL and its
# arguments in $MARK, then exits STATUS.
stub() {
  mkdir -p "$1/formal/scripts"
  cat >"$1/formal/scripts/verify-artifacts.sh" <<EOF
echo "$2 \$*" >>"\$MARK"
exit $3
EOF
}

# run_case NAME EXPECT(pass|fail) SETUP...: lays out a workspace, runs the
# step, prints the recorded verifier calls to $tmp/NAME.calls.
fail=0
run_case() {
  local name="$1" expect="$2" ws status=0
  ws="$tmp/$name"
  mkdir -p "$ws/untrusted/formal" "$ws/trusted/formal" "$ws/runner"
  : >"$ws/calls"
  stub "$ws/untrusted" untrusted 0
  case "$name" in
    no-trusted) ;;
    trusted-pass) stub "$ws/trusted" trusted 0 ;;
    trusted-fail) stub "$ws/trusted" trusted 1 ;;
  esac
  (
    cd "$ws"
    MARK="$ws/calls" RUNNER_TEMP="$ws/runner" SYSROOT="$ws/sysroot" \
      bash --noprofile --norc -eo pipefail "$tmp/step.sh"
  ) >"$ws/out" 2>&1 || status=$?
  if [[ "$expect" == pass && "$status" != 0 ]] ||
     [[ "$expect" == fail && "$status" == 0 ]]; then
    echo "SELFTEST FAIL: $name: expected $expect, exit status $status" >&2
    sed 's/^/  | /' "$ws/out" >&2
    fail=1
  fi
  if grep -q '^untrusted' "$ws/calls"; then
    echo "SELFTEST FAIL: $name: the pull request's verifier ran" >&2
    fail=1
  fi
}

run_case no-trusted fail
if [[ -s "$tmp/no-trusted/calls" ]]; then
  echo "SELFTEST FAIL: no-trusted: a verifier ran" >&2; fail=1
fi
if ! grep -q '::error::' "$tmp/no-trusted/out"; then
  echo "SELFTEST FAIL: no-trusted: no ::error:: annotation" >&2; fail=1
fi

run_case trusted-pass pass
calls="$(cat "$tmp/trusted-pass/calls")"
for want in '^trusted ' '--allowlist trusted/formal/axioms-allowlist.txt' \
  '--base-lakefile trusted/formal/lakefile.toml' '--sources untrusted/formal'; do
  if ! grep -q -e "$want" <<<"$calls"; then
    echo "SELFTEST FAIL: trusted-pass: verifier call lacks '$want': $calls" >&2
    fail=1
  fi
done

run_case trusted-fail fail

if [[ "$fail" != 0 ]]; then
  echo "workflow self-test FAILED" >&2
  exit 1
fi
echo "workflow self-test passed (3 cases)"
