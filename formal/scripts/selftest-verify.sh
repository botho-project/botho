#!/usr/bin/env bash
# Self-test for the trusted verify job (#1799).
#
# For the clean tree and each fixture under fixtures/verify/ this script:
#   1. copies the formal/ tree to a scratch directory and applies the fixture
#      (the pull request);
#   2. runs `lake build` there, which runs the fixture's code, as the
#      untrusted `build` job does;
#   3. keeps only the files the workflow uploads (artifact_files below must
#      match the `upload-artifact` paths in .github/workflows/formal.yml);
#   4. runs verify-artifacts.sh from this directory with the clean
#      allow-list, as the verify job runs the base branch's copy, from a
#      directory that holds a canary `axioms-allowlist.txt`.
#
# It asserts that the clean tree passes; that each fixture fails, for the
# reason in its expect.txt; that the canary allow-list is unchanged; and that
# no model initializer ran inside the verifier (the fixtures' initializers
# write to $BOTHO_INIT_CANARY). As a control it then imports the fixture's
# library through the Lean frontend (`import Botho`), which does run
# initializers, and checks the canary appears there. That shows the probe is
# live and that the verifier's not running initializers is what keeps them
# out.
#
# Usage: selftest-verify.sh [FIXTURE ...]   (default: clean + all fixtures)
# Environment: LAKE overrides the lake binary (default: lake); LEAN_SYSROOT
# the toolchain directory (default: `lean --print-prefix`).
set -euo pipefail

main() {
local here root lake sysroot
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="$(cd "$here/.." && pwd)"
lake="${LAKE:-lake}"
sysroot="${LEAN_SYSROOT:-$(lean --print-prefix)}"
local fixtures="$here/fixtures/verify"
local cases=("$@")
if [[ ${#cases[@]} -eq 0 ]]; then
  cases=(clean)
  local d
  for d in "$fixtures"/*/; do cases+=("$(basename "$d")"); done
fi

local tmp
tmp="$(mktemp -d)"
# shellcheck disable=SC2064  # expand now: tmp is local
trap "rm -rf '$tmp'" EXIT
local fail=0 c
for c in "${cases[@]}"; do
  if ! run_case "$c"; then
    echo "SELFTEST FAIL: $c" >&2
    fail=1
  fi
done
if [[ "$fail" != 0 ]]; then
  echo "FAIL: verify self-test" >&2
  exit 1
fi
echo "OK: verify self-test passed (${#cases[@]} cases)"
}

# The files the build job uploads, relative to .lake/build/lib/lean. Keep in
# sync with the `upload-artifact` step in .github/workflows/formal.yml.
artifact_files() {
  (cd "$1" && find . -type f \( -name '*.olean' -o -name '*.olean.server' \
    -o -name '*.olean.private' -o -name '*.ir' \) | sed 's|^\./||' | LC_ALL=C sort)
}

run_case() {
  local name="$1" dir="$tmp/$1" fx=""
  echo "== verify self-test: $name"
  mkdir -p "$dir/tree" "$dir/artifact" "$dir/cwd"
  local f
  for f in Botho.lean Botho Theorems.lean lakefile.toml lean-toolchain \
    lake-manifest.json axioms-allowlist.txt; do
    cp -R "$root/$f" "$dir/tree/$f"
  done
  if [[ "$name" != clean ]]; then
    fx="$fixtures/$name"
    if [[ ! -d "$fx" ]]; then
      echo "no fixture $fx" >&2
      return 1
    fi
    if [[ -d "$fx/Botho" ]]; then cp -R "$fx/Botho/." "$dir/tree/Botho/"; fi
    if [[ -f "$fx/imports.txt" ]]; then cat "$fx/imports.txt" >>"$dir/tree/Botho.lean"; fi
    if [[ -f "$fx/manifest.txt" ]]; then cat "$fx/manifest.txt" >>"$dir/tree/Theorems.lean"; fi
    if [[ -f "$fx/allowlist.txt" ]]; then
      cat "$fx/allowlist.txt" >>"$dir/tree/axioms-allowlist.txt"
    fi
    if [[ -f "$fx/srcdir" ]]; then
      mkdir "$dir/tree/src"
      mv "$dir/tree/Botho.lean" "$dir/tree/Botho" "$dir/tree/src/"
      printf 'srcDir = "src"\n' >>"$dir/tree/lakefile.toml"
    fi
  fi

  # Untrusted build: the fixture's code runs here.
  if ! (cd "$dir/tree" && "$lake" build) >"$dir/build.log" 2>&1; then
    cat "$dir/build.log"
    echo "lake build failed for $name" >&2
    return 1
  fi
  local lib="$dir/tree/.lake/build/lib/lean" rel
  while IFS= read -r rel; do
    mkdir -p "$dir/artifact/$(dirname "$rel")"
    cp "$lib/$rel" "$dir/artifact/$rel"
  done < <(artifact_files "$lib")
  # A fixture may also plant extra files in the artifact (a build job can
  # upload anything), for example an Init/Prelude.olean to shadow the
  # toolchain's.
  if [[ -n "$fx" && -f "$fx/artifact-extra.txt" ]]; then
    while IFS= read -r rel; do
      mkdir -p "$dir/artifact/$(dirname "$rel")"
      cp "$lib/Botho.olean" "$dir/artifact/$rel"
    done <"$fx/artifact-extra.txt"
  fi

  # Trusted verify: this directory's scripts and the clean allow-list.
  # Run 1 (as CI): the whole verifier, source scan included. Run 2
  # (--no-scan): the artifact checks alone, which must reject the fixture for
  # the reason in expect.txt whatever the base branch's scan catches first.
  local canary="$dir/init-canary" ok=1 expect="" run rc log
  if [[ -n "$fx" ]]; then expect="$(cat "$fx/expect.txt")"; fi
  for run in full no-scan; do
    if [[ "$name" == clean && "$run" == no-scan ]]; then continue; fi
    log="$dir/verify-$run.log"
    rc=0
    printf 'propext\n' >"$dir/cwd/axioms-allowlist.txt"
    local extra=()
    if [[ "$run" == no-scan ]]; then extra=(--no-scan); fi
    (cd "$dir/cwd" && BOTHO_INIT_CANARY="$canary" \
      bash "$here/verify-artifacts.sh" --lib "$dir/artifact" --sources "$dir/tree" \
        --sysroot "$sysroot" --allowlist "$root/axioms-allowlist.txt" \
        ${extra[@]+"${extra[@]}"}) >"$log" 2>&1 || rc=$?
    if [[ "$name" == clean ]]; then
      if [[ "$rc" != 0 ]]; then
        echo "  expected the clean tree to pass (rc=$rc)" >&2
        ok=0
      fi
    elif [[ "$rc" == 0 ]]; then
      echo "  [$run] expected verify to fail, it passed" >&2
      ok=0
    elif [[ "$run" == no-scan ]] && ! grep -Eq -- "$expect" "$log"; then
      echo "  [$run] verify failed, but not for the expected reason: $expect" >&2
      ok=0
    fi
    if [[ "$(cat "$dir/cwd/axioms-allowlist.txt")" != propext ]]; then
      echo "  [$run] the canary allow-list in the verifier's directory was rewritten" >&2
      ok=0
    fi
    if [[ -e "$canary" ]]; then
      echo "  [$run] a model initializer ran inside the verifier" >&2
      ok=0
    fi
    if [[ -e "$dir/cwd/PWNED_INIT" ]]; then
      echo "  [$run] PWNED_INIT appeared in the verifier's directory" >&2
      ok=0
    fi
    if [[ "$ok" != 1 ]]; then
      echo "  --- verify output ($run):" >&2
      sed 's/^/  | /' "$log" >&2
      return 1
    fi
  done

  # Control: the frontend import runs the same initializers.
  if [[ -n "$fx" ]] && grep -rqs 'BOTHO_INIT_CANARY' "$fx"; then
    mkdir "$dir/ctl"
    printf 'import Botho\n' >"$dir/ctl/Ctl.lean"
    printf 'propext\n' >"$dir/ctl/axioms-allowlist.txt"
    (cd "$dir/ctl" && BOTHO_INIT_CANARY="$canary" LEAN_PATH="$dir/artifact" \
      "$sysroot/bin/lean" Ctl.lean) >"$dir/ctl.log" 2>&1 || true
    if [[ ! -e "$canary" ]]; then
      echo "  control: importing the library did not run its initializer;" \
        "the canary probe is not live" >&2
      ok=0
    else
      echo "  control: \`import Botho\` through the frontend ran the initializer (as expected)"
      if [[ "$(cat "$dir/ctl/axioms-allowlist.txt")" != propext ]]; then
        echo "  control: ... and it rewrote the allow-list in its directory"
      fi
    fi
  fi

  if [[ "$ok" != 1 ]]; then
    return 1
  fi
  if [[ "$name" == clean ]]; then
    echo "  ok: clean tree passes"
  else
    echo "  ok: rejected by the full verifier ($(grep -m1 '^FAIL' "$dir/verify-full.log"))"
    echo "  ok: rejected by the artifact checks alone ($(grep -E -m1 -- "$expect" "$dir/verify-no-scan.log"))"
  fi
}

main "$@"
