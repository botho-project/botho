#!/usr/bin/env bash
# Propose statement-record blocks for the formal model (#1805).
#
# Builds the tree with `lake build`, runs AxiomReport.lean over the compiled
# library for every theorem listed in Theorems.lean, and prints the
# statements.txt those theorems compile to (statements.py render). This is
# a helper for authors and reviewers, and the build job runs it for
# information. It decides nothing: the verify job reads the BASE commit's
# statements.txt and recomputes each block itself (verify-artifacts.sh).
#
# `lake build` runs model code, so the output is only as honest as the tree
# you build. A reviewer who checks a proposed block builds a tree they have
# reviewed (no build-time code; see "Limits" in formal/README.md) and
# compares the printed text with the sources.
#
# Usage: propose-statements.sh [--check | --write] [FORMAL_ROOT]
#   (no flag)  print the computed record on stdout.
#   --check    compare it with FORMAL_ROOT/statements.txt: exit 1 if a
#              listed theorem has no identical block there, and list the
#              blocks that match no compiled theorem (pending or stale).
#   --write    overwrite FORMAL_ROOT/statements.txt with the computed
#              record (drops pending and stale blocks).
# Environment: LAKE overrides the lake binary (default: lake).
set -euo pipefail

main() {
local mode=print
while [[ $# -gt 0 ]]; do
  case "$1" in
    --check) mode="check"; shift ;;
    --write) mode="write"; shift ;;
    -*) echo "usage: $0 [--check | --write] [FORMAL_ROOT]" >&2; exit 2 ;;
    *) break ;;
  esac
done
here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="${1:-$(cd "$here/.." && pwd)}"
root="$(cd "$root" && pwd)"
lake="${LAKE:-lake}"
gate_lib=Botho
# shellcheck disable=SC1091  # gate-common.sh sits next to this script
. "$here/gate-common.sh"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

# The manifest's theorem names (the gates check its syntax strictly; this
# helper only needs the names).
sed -n -E 's/^[[:space:]]*#print[[:space:]]+axioms[[:space:]]+([^[:space:]]+)[[:space:]]*$/\1/p' \
  "$root/Theorems.lean" | sed -E 's/^(_root_\.)+//' | awk '!seen[$0]++' >"$tmp/names"
if [[ ! -s "$tmp/names" ]]; then
  echo "FAIL: no '#print axioms' lines in $root/Theorems.lean" >&2
  exit 1
fi
local names=()
while IFS= read -r n; do names+=("$n"); done <"$tmp/names"

if ! (cd "$root" && "$lake" build) >"$tmp/build.log" 2>&1; then
  cat "$tmp/build.log" >&2
  echo "FAIL: lake build failed" >&2
  exit 1
fi
run_report "$tmp/report" "${names[@]}"

case "$mode" in
  print) python3 -I "$here/statements.py" render "$tmp/report" "$tmp/names" ;;
  write)
    python3 -I "$here/statements.py" render "$tmp/report" "$tmp/names" >"$tmp/statements.txt"
    cp "$tmp/statements.txt" "$root/statements.txt"
    echo "wrote $root/statements.txt (${#names[@]} theorems)" ;;
  check) python3 -I "$here/statements.py" diff "$tmp/report" "$root/statements.txt" "$tmp/names" ;;
esac
}

main "$@"
