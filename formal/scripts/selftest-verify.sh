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
# It asserts that the clean tree passes, also with no `lean`, `lake` or `elan`
# on PATH (the verify job installs none there); that each fixture fails, for
# the reason in its expect.txt, or passes if it is marked `pass`; that the
# canary allow-list is unchanged; and that
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
sysroot="$(cd "$sysroot" && pwd)"
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

# A PATH with python3 and the system tools but no lean, lake, leanchecker or
# elan, for the no-lean-path run. Fails if it cannot build one.
lean_free_path() {
  local bin="$tmp/lean-free-bin" py t
  if [[ ! -d "$bin" ]]; then
    mkdir "$bin"
    py="$(command -v python3)"
    ln -s "$py" "$bin/python3"
  fi
  local p="$bin:/usr/bin:/bin"
  for t in lean lake leanchecker elan; do
    if PATH="$p" command -v "$t" >/dev/null 2>&1; then
      echo "lean_free_path: $t is still on $p" >&2
      return 1
    fi
  done
  echo "$p"
}

# The decoy layout (see fixtures/verify/README.md): copy Botho.lean and
# Botho/ to src/ and, in that copy only, replace each listed theorem (from
# `theorem NAME` up to the next blank line) with `theorem NAME : True :=
# trivial`. decoy.txt lines: FILE NAME, FILE relative to the tree.
make_decoy() {
  local tree="$1" spec="$2"
  mkdir "$tree/src"
  cp -R "$tree/Botho.lean" "$tree/Botho" "$tree/src/"
  python3 -I - "$tree/src" "$spec" <<'PY'
import os
import sys

src, spec = sys.argv[1:]
with open(spec, encoding="utf-8") as fh:
    rows = [r.split() for r in fh.read().split("\n") if r.strip()]
for path, name in rows:
    p = os.path.join(src, path)
    with open(p, encoding="utf-8") as fh:
        s = fh.read()
    i = s.find(f"theorem {name} ")
    k = s.find("\n\n", i)
    if i < 0 or k < 0:
        sys.exit(f"decoy: no theorem {name} followed by a blank line in {path}")
    s = s[:i] + f"theorem {name} : True := trivial" + s[k:]
    with open(p, "w", encoding="utf-8") as fh:
        fh.write(s)
PY
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
    if [[ -f "$fx/decoy.txt" ]]; then
      make_decoy "$dir/tree" "$fx/decoy.txt" || return 1
    fi
    if [[ -f "$fx/lakefile.toml" ]]; then cp "$fx/lakefile.toml" "$dir/tree/lakefile.toml"; fi
    if [[ -f "$fx/lakefile-append.txt" ]]; then
      cat "$fx/lakefile-append.txt" >>"$dir/tree/lakefile.toml"
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

  # Trusted verify: this directory's scripts, the clean allow-list and the
  # clean lakefile.toml as the base's.
  # Run full (as CI): the whole verifier, source scan included.
  # Run no-scan: the other checks alone, which must reject the fixture for the
  # reason in expect.txt whatever the base branch's scan catches first.
  # Run no-lean-path (clean tree only): as full, with no lean, lake or elan on
  # PATH and LEAN_SYSROOT unset, as in the verify job: --sysroot alone must
  # supply the toolchain.
  # Run same-layout (if the fixture has expect-same-layout.txt): as no-scan,
  # but with the fixture's own lakefile.toml as the base's, so the layout
  # check passes. Its output must match expect-same-layout.txt. This shows
  # what the fixture would get past without the layout check.
  local canary="$dir/init-canary" ok=1 expect="" pass=0 run rc log
  local runs=(full no-scan)
  if [[ "$name" == clean ]]; then
    pass=1
    runs=(full no-lean-path)
  elif [[ -f "$fx/pass" ]]; then
    pass=1
  else
    expect="$(cat "$fx/expect.txt")"
  fi
  if [[ -n "$fx" && -f "$fx/expect-same-layout.txt" ]]; then runs+=(same-layout); fi
  for run in "${runs[@]}"; do
    log="$dir/verify-$run.log"
    rc=0
    printf 'propext\n' >"$dir/cwd/axioms-allowlist.txt"
    local extra=() base_lakefile="$root/lakefile.toml" env_cmd=(env)
    case "$run" in
      no-scan) extra=(--no-scan) ;;
      same-layout) extra=(--no-scan); base_lakefile="$dir/tree/lakefile.toml" ;;
      no-lean-path)
        local nolean
        nolean="$(lean_free_path)" || return 1
        env_cmd=(env -u LEAN_SYSROOT -u LEAN_PATH PATH="$nolean") ;;
    esac
    (cd "$dir/cwd" && "${env_cmd[@]}" BOTHO_INIT_CANARY="$canary" \
      bash "$here/verify-artifacts.sh" --lib "$dir/artifact" --sources "$dir/tree" \
        --sysroot "$sysroot" --allowlist "$root/axioms-allowlist.txt" \
        --base-lakefile "$base_lakefile" ${extra[@]+"${extra[@]}"}) >"$log" 2>&1 || rc=$?
    if [[ "$run" == same-layout ]]; then
      if ! grep -Eq -- "$(cat "$fx/expect-same-layout.txt")" "$log"; then
        echo "  [$run] output does not match expect-same-layout.txt:" \
          "$(cat "$fx/expect-same-layout.txt")" >&2
        ok=0
      fi
    elif [[ "$pass" == 1 ]]; then
      if [[ "$rc" != 0 ]]; then
        echo "  [$run] expected verify to pass (rc=$rc)" >&2
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
    (cd "$dir/ctl" && BOTHO_INIT_CANARY="$canary" LEAN_SYSROOT="$sysroot" \
      LEAN_PATH="$dir/artifact" "$sysroot/bin/lean" Ctl.lean) >"$dir/ctl.log" 2>&1 || true
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
    echo "  ok: clean tree passes, also with no lean, lake or elan on PATH"
  elif [[ "$pass" == 1 ]]; then
    echo "  ok: accepted by the full verifier and by the checks without the scan"
  else
    echo "  ok: rejected by the full verifier ($(grep -m1 '^FAIL' "$dir/verify-full.log"))"
    echo "  ok: rejected without the source scan ($(grep -E -m1 -- "$expect" "$dir/verify-no-scan.log"))"
  fi
  if [[ -f "$dir/verify-same-layout.log" ]]; then
    echo "  ok: with its own lakefile as the base's: $(grep -E -m1 -- \
      "$(cat "$fx/expect-same-layout.txt")" "$dir/verify-same-layout.log")"
  fi
}

main "$@"
