#!/usr/bin/env bash
# Fresh loopback ledger only. No user wallet or live RPC configuration is read.
set -euo pipefail
cd "$(dirname "${BASH_SOURCE[0]}")/.."
node localnet/verify-artifact.cjs
[[ -f target/deploy/wbth.so && -f target/idl/wbth.json ]] || {
    echo 'Run anchor build first (Solana 1.18.26, Anchor 0.29.0).' >&2; exit 1;
}
[[ "$(solana-test-validator --version)" == 'solana-test-validator 1.18.26 '* ]] || {
    echo 'This harness requires solana-test-validator 1.18.26 on PATH.' >&2; exit 1;
}
run_dir="$(mktemp -d "${TMPDIR:-/tmp}/botho-squads.XXXXXX")"
validator_pid=""
if curl --silent --max-time 2 -H 'Content-Type: application/json' \
    --data '{"jsonrpc":"2.0","id":1,"method":"getVersion"}' http://127.0.0.1:18899 >/dev/null; then
    echo 'Port 18899 is already serving an RPC; refusing to reuse its state.' >&2; exit 1
fi
cleanup() {
    if [[ -n "$validator_pid" ]]; then
        kill "$validator_pid" 2>/dev/null || true
        wait "$validator_pid" 2>/dev/null || true
    fi
    echo "Local harness logs and genesis fixture: $run_dir"
}
trap cleanup EXIT
config_pda="$(node --import tsx localnet/squads.ts genesis "$run_dir/config.json")"
solana-test-validator --reset --quiet --ledger "$run_dir/ledger" \
    --bind-address 127.0.0.1 --rpc-port 18899 --faucet-port 18925 \
    --gossip-port 18901 --dynamic-port-range 18902-18922 \
    --mint AKnL4NNf3DGWZJS6cPknBuEGnVsV4A4m5tgebLHaRSZ9 \
    --account "$config_pda" "$run_dir/config.json" \
    --bpf-program SQDS4ep65T869zMMBKyuUq6aD6EgTu8psMjkvj52pCf fixtures/squads-v4/squads_multisig_program.so \
    --bpf-program CZDnzeywrqEM5ereWJmtYKUQ9uJXxX2PydqqKTQStxxE target/deploy/wbth.so \
    >"$run_dir/validator.log" 2>&1 &
validator_pid=$!
ready=0
for _ in $(seq 1 60); do
    if ! kill -0 "$validator_pid" 2>/dev/null; then cat "$run_dir/validator.log" >&2; exit 1; fi
    if curl --silent --fail -H 'Content-Type: application/json' \
        --data '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' http://127.0.0.1:18899 | grep -q '"ok"'; then
        ready=1; break
    fi
    sleep 1
done
[[ "$ready" == 1 ]] || { cat "$run_dir/validator.log" >&2; exit 1; }
node --import tsx localnet/squads.ts test "${SQUADS_EVIDENCE_PATH:-$run_dir/evidence.json}"
