#!/usr/bin/env bash
# Exercise cold driver startup without compilers, nodes, credentials or networks.
set -euo pipefail
REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEST_ROOT"' EXIT
mkdir -p "$TEST_ROOT/bin" "$TEST_ROOT/repo/scripts" \
    "$TEST_ROOT/repo/contracts/ethereum/node_modules"

cat > "$TEST_ROOT/bin/cargo" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
if [[ "$1" == build ]]; then
    echo build >> "$CALLS"
    release=0; node=0; harness=0
    while [[ $# -gt 0 ]]; do
        case "$1" in
            --release) release=1 ;;
            --bin)
                shift
                case "$1" in botho) node=1 ;; botho-testnet) harness=1 ;; esac
                ;;
        esac
        shift
    done
    [[ "$release$node$harness" == 111 ]] || exit 91
    [[ "$FAIL_BUILD" == 0 ]] || exit 42
    mkdir -p "$CARGO_TARGET_DIR/release"
    touch "$CARGO_TARGET_DIR/release/botho" "$CARGO_TARGET_DIR/release/botho-testnet"
    exit 0
fi
if [[ "$*" == *" -- start "* ]]; then
    echo start >> "$CALLS"
    [[ -f "$CARGO_TARGET_DIR/release/botho" ]] || exit 92
    [[ -f "$CARGO_TARGET_DIR/release/botho-testnet" ]] || exit 93
    # Stop exactly at the process boundary; no mining/provisioning is simulated.
    exit 43
fi
echo unexpected-cargo >> "$CALLS"
exit 94
STUB
cat > "$TEST_ROOT/bin/npx" <<'STUB'
#!/usr/bin/env bash
[[ "$*" == 'hardhat compile' ]] || exit 95
STUB
chmod +x "$TEST_ROOT/bin/cargo" "$TEST_ROOT/bin/npx"

for driver in bridge-e2e-full-loop.sh bridge-e2e-defi-fork.sh bridge-e2e-defi-solana.sh; do
    cp "$REPO_ROOT/scripts/$driver" "$TEST_ROOT/repo/scripts/"
    for fail in 0 1; do
        target_dir="$TEST_ROOT/artifacts-$driver-$fail"
        calls="$TEST_ROOT/calls-$driver-$fail"
        : > "$calls"
        status=0
        env PATH="$TEST_ROOT/bin:$PATH" CARGO_TARGET_DIR="$target_dir" \
            CALLS="$calls" FAIL_BUILD="$fail" \
            BRIDGE_FORK_RPC_URL=http://127.0.0.1:18545 \
            BRIDGE_SOLANA_RPC_URL=http://127.0.0.1:18899 RUN_LOCAL_VALIDATOR=0 \
            bash "$TEST_ROOT/repo/scripts/$driver" > "$TEST_ROOT/output" 2>&1 || status=$?
        if [[ "$fail" == 0 ]]; then
            expected=$'build\nstart'
            expected_status=43
        else
            expected=build
            expected_status=42
        fi
        if [[ "$(cat "$calls")" != "$expected" || "$status" != "$expected_status" ]]; then
            echo "FAIL $driver build_failure=$fail (status=$status)" >&2
            cat "$calls" "$TEST_ROOT/output" >&2
            exit 1
        fi
        echo "PASS $driver build_failure=$fail"
    done
done
