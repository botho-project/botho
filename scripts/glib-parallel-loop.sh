#!/usr/bin/env bash
# Loop the vendored GLib lib-test binary under parallel test threads to catch
# the intermittent "failed to initiate panic, error 3" abort (#1561).
#
# The Security workflow runs these tests with `--test-threads=1` because of
# that abort. This script re-enables parallelism and runs each iteration under
# gdb. When an iteration dies on a signal, gdb prints `info threads` and
# `thread apply all bt`, and the script extracts the `::tests::` frames, so the
# backtraces show which tests were in flight at the abort. As a fallback the
# script also lists the tests that never reported a result, worked out from
# `--list` minus the completed `test <name> ... <result>` lines.
#
# Usage:
#   scripts/glib-parallel-loop.sh --binary PATH [--iterations N]
#       [--threads N] [--out DIR] [--no-gdb]
#
#   --binary      Built lib-test executable, for example from
#                 `cargo test --manifest-path vendor/glib/Cargo.toml --release
#                  --locked --lib --no-run --message-format=json`.
#   --iterations  Number of runs (default 50).
#   --threads     Value for `--test-threads` (default: let libtest decide).
#   --out         Directory for per-failure logs (default ./glib-loop-out).
#   --no-gdb      Run the binary directly. Faster, but a failure then only
#                 reports the unfinished-test list.
#
# Run it from vendor/glib: some upstream tests read fixtures relative to the
# crate root, the same way `cargo test` runs them.
#
# Exit status: 0 if every iteration passed, 1 if any failed, 2 on bad usage.

set -euo pipefail

binary=""
iterations=50
threads=""
out="glib-loop-out"
use_gdb=1

while [[ $# -gt 0 ]]; do
    case "$1" in
        --binary) binary="$2"; shift 2 ;;
        --iterations) iterations="$2"; shift 2 ;;
        --threads) threads="$2"; shift 2 ;;
        --out) out="$2"; shift 2 ;;
        --no-gdb) use_gdb=0; shift ;;
        -h|--help) sed -n '2,30p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

if [[ -z "$binary" || ! -x "$binary" ]]; then
    echo "--binary must name an executable lib-test binary" >&2
    exit 2
fi
if ! [[ "$iterations" =~ ^[0-9]+$ ]] || [[ "$iterations" -lt 1 ]]; then
    echo "--iterations must be a positive integer" >&2
    exit 2
fi
if [[ -n "$threads" ]] && ! [[ "$threads" =~ ^[0-9]+$ ]]; then
    echo "--threads must be a positive integer" >&2
    exit 2
fi
if [[ "$use_gdb" -eq 1 ]] && ! command -v gdb >/dev/null 2>&1; then
    echo "gdb not found; install it or pass --no-gdb" >&2
    exit 2
fi

mkdir -p "$out"
export RUST_BACKTRACE=1

test_args=()
if [[ -n "$threads" ]]; then
    test_args+=("--test-threads=$threads")
fi

all_tests="$out/all-tests.txt"
"$binary" --list --format=terse 2>/dev/null | sed -n 's/: test$//p' | sort > "$all_tests"
echo "binary: $binary"
echo "tests: $(wc -l < "$all_tests" | tr -d ' ')  iterations: $iterations  threads: ${threads:-default}  gdb: $use_gdb"

failures=0
for ((i = 1; i <= iterations; i++)); do
    log="$out/iter-$i.log"
    failed=0
    if [[ "$use_gdb" -eq 1 ]]; then
        # gdb stops on SIGABRT/SIGSEGV/SIGBUS by default. After a clean exit
        # the follow-up commands fail harmlessly ("No stack").
        gdb -q -batch -nx \
            -ex 'set pagination off' \
            -ex 'set print thread-events off' \
            -ex 'set confirm off' \
            -ex run \
            -ex 'info threads' \
            -ex 'thread apply all bt' \
            --args "$binary" ${test_args[@]+"${test_args[@]}"} > "$log" 2>&1 || true
        if grep -q 'received signal' "$log" || ! grep -q '^test result: ok\.' "$log"; then
            failed=1
        fi
    else
        if ! "$binary" ${test_args[@]+"${test_args[@]}"} > "$log" 2>&1; then
            failed=1
        fi
    fi

    if [[ "$failed" -eq 1 ]]; then
        failures=$((failures + 1))
        echo "::group::iteration $i FAILED"
        grep -E 'fatal runtime error|received signal|panicked at' "$log" | head -20 || true
        if [[ "$use_gdb" -eq 1 ]]; then
            # Linux truncates thread names to 15 bytes, so also pull the test
            # functions out of the backtraces.
            echo "-- threads at the signal:"
            grep -E '^\*? +[0-9]+ +Thread ' "$log" || true
            echo "-- test functions on any stack at the signal:"
            grep -oE '[A-Za-z0-9_:]+::tests?::[A-Za-z0-9_]+' "$log" | sort -u || true
        fi
        echo "-- tests that never reported a result:"
        sed -nE 's/^test (.+) \.\.\. .*/\1/p' "$log" | sort -u > "$out/iter-$i.done.txt"
        comm -23 "$all_tests" "$out/iter-$i.done.txt" | tee "$out/iter-$i.unfinished.txt"
        echo "::endgroup::"
    else
        rm -f "$log"
    fi
    if (( i % 10 == 0 )); then
        echo "progress: $i/$iterations, failures: $failures"
    fi
done

echo "done: $iterations iterations, $failures failure(s); logs for failures in $out"
[[ "$failures" -eq 0 ]]
