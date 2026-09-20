#!/usr/bin/env bash
# Execute every ledger unit test without exhausting macOS SysV undo entries.
set -euo pipefail

usage() {
  echo "Usage: $0 [--binary /absolute/path/to/botho-lib-test]"
  echo "Otherwise uses cargo test --locked -p botho --lib ledger::."
}

binary=""
case "${1:-}" in
  "") ;;
  --help|-h) usage; exit 0 ;;
  --binary)
    if [[ $# -ne 2 || "$2" != /* || ! -f "$2" || ! -x "$2" ]]; then
      echo "--binary requires one absolute executable file path" >&2
      exit 2
    fi
    binary="$2"
    ;;
  *) usage >&2; exit 2 ;;
esac

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$root"
args=(ledger:: --nocapture)
if [[ $(uname -s) == Darwin ]]; then
  # The reviewed ledger tests hold at most one writer plus a transient reader
  # lock per test. Leave two entries for incidental work; never change sysctl.
  limit=$(sysctl -n kern.sysv.semume) || {
    echo "Cannot read kern.sysv.semume; no ledger tests started" >&2; exit 2;
  }
  if [[ ! "$limit" =~ ^[1-9][0-9]{0,8}$ ]] || (( limit < 4 )); then
    echo "Unsupported kern.sysv.semume=$limit: need at least 4 for one test plus headroom" >&2
    exit 2
  fi
  threads=$(( (limit - 2) / 2 ))
  if (( threads > 4 )); then threads=4; fi
  requested="${RUST_TEST_THREADS:-}"
  if [[ -n "$requested" ]]; then
    if [[ ! "$requested" =~ ^[1-9][0-9]{0,8}$ ]]; then
      echo "RUST_TEST_THREADS must be a positive decimal integer" >&2; exit 2
    fi
    if (( requested < threads )); then threads=$requested; fi
  fi
  echo "macOS ledger tests: semume=$limit; workers=$threads (cap 4, two entries/test plus two headroom)" >&2
  args+=("--test-threads=$threads")
fi

if [[ -n "$binary" ]]; then
  exec "$binary" "${args[@]}"
fi
# Cargo's filter is before --; the remaining options are libtest-only.
exec cargo test --locked -p botho --lib "${args[0]}" -- "${args[@]:1}"
