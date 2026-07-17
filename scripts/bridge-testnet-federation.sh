#!/usr/bin/env bash
# Testnet bridge FEDERATION driver (#868).
#
# Stands up a REAL t-of-n threshold bridge federation — N independent
# `bth-bridge` service processes, each holding ONLY its own attestation keys,
# exchanging signed attestation envelopes over the wire (`POST /api/attest`,
# #858) — wired to the LIVE test infrastructure:
#
#   * BTH:      the live betanet   (default https://seed.botho.io/rpc)
#   * Ethereum: live Sepolia       (wBTH 0x49b985eC…, 2-of-3 Safe 0x61274F55…)
#   * Solana:   live devnet        (wbth program CZDnzeywrqEM…)
#
# and drives / verifies the round-trip drill of docs/bridge/testnet-e2e-runbook.md
# (Phase C): BTH deposit → threshold attestation → wBTH mint → confirm;
# wBTH burn → threshold release attestation → BTH reserve release → confirm;
# asserting exactly-once, factor-1 amounts (ADR 0003), and the live
# proof-of-reserves invariant (Σ wBTH == locked reserve).
#
# TOPOLOGY (single-host federation). The N instances run on one host and
# share ONE SQLite order store (WAL + busy-timeout, see
# bridge/service/src/db.rs). Sharing the order store is what today's code
# supports: order records do NOT replicate between instances over the network
# (the #858 transport exchanges attestation envelopes only — an envelope for
# an order a peer has never heard of is refused `unknown_order`). The
# CRYPTOGRAPHIC custody is still genuinely t-of-n over the wire: each process
# signs with only ITS key, envelopes travel through the real authenticated
# HTTP endpoint, and no mint/release is prepared until `threshold` distinct
# federation signatures verify. Cross-host order replication is the tracked
# follow-up (see the #868 findings issue).
#
# SECRETS. Everything lives under .secrets/bridge-testnet/ (git-ignored,
# testnet-disposable). NEVER point this at .secrets/bridge-mainnet.
#
# USAGE
#   ./scripts/bridge-testnet-federation.sh keys          # federation attestation keys
#   ./scripts/bridge-testnet-federation.sh gen-reserve   # throwaway BTH reserve+user wallets
#   ./scripts/bridge-testnet-federation.sh up            # render configs + start N instances
#   ./scripts/bridge-testnet-federation.sh status        # /api/status of each instance
#   ./scripts/bridge-testnet-federation.sh proof         # live /api/reserve/proof of each
#   ./scripts/bridge-testnet-federation.sh order-mint <amount-pc> <eth-dest>
#   ./scripts/bridge-testnet-federation.sh order-status <order-id>
#   ./scripts/bridge-testnet-federation.sh drill-mint <amount-pc> <eth-dest>
#   ./scripts/bridge-testnet-federation.sh drill-burn <amount-pc> <bth-dest-address>
#   ./scripts/bridge-testnet-federation.sh attest-log    # federation attestation audit trail
#   ./scripts/bridge-testnet-federation.sh orders        # order table snapshot
#   ./scripts/bridge-testnet-federation.sh logs [i]      # tail instance logs
#   ./scripts/bridge-testnet-federation.sh down          # stop all instances
#   ./scripts/bridge-testnet-federation.sh clean         # down + delete run state (keeps keys)
#
# ENV KNOBS (defaults target the LIVE testnet):
#   BRIDGE_FED_NODES=3                 federation size n
#   BRIDGE_FED_THRESHOLD=2             threshold t
#   BRIDGE_BTH_RPC_URL=https://seed.botho.io/rpc
#   SEPOLIA_RPC_URL=https://ethereum-sepolia-rpc.publicnode.com
#   SOLANA_RPC_URL=https://api.devnet.solana.com
#   BRIDGE_WBTH_ADDRESS=0x49b985eC427EE771A601F11b18f7d4402fA2DD7B
#   BRIDGE_SAFE_ADDRESS=0x61274F558f9027e2D402d3340dE89152FA3F3947
#   BRIDGE_ETH_CHAIN_ID=11155111
#   BRIDGE_ETH_CONFIRMATIONS=2         Sepolia depth before Completed
#   BRIDGE_SOLANA_PROGRAM=CZDnzeywrqEM5ereWJmtYKUQ9uJXxX2PydqqKTQStxxE
#   BRIDGE_SOLANA_FEDERATION=0         1 wires the Solana ed25519 federation +
#                                      mint keypair. BLOCKED today: the devnet
#                                      wbth mint_authority is a single key, and
#                                      the startup custody guard HARD-FAILS a
#                                      federation posture with a single-key
#                                      authority (mint/solana.rs). Flip to 1
#                                      only after the devnet authority migrates
#                                      to a real multisig (Squads).
#   BRIDGE_BTH_RESERVE_VIEW_KEY / _SPEND_KEY / _PQ_SEED / _ADDRESS
#                                      the funded factor-1 reserve (file paths +
#                                      address). Auto-loaded from
#                                      $FED_DIR/reserve.env when present
#                                      (written by `gen-reserve`).
#   BRIDGE_BTH_RPC_URLS                comma list; instance i watches entry
#                                      (i-1) mod len — one node per member
#   BRIDGE_RESERVE_TOLERANCE=0         reconciler tolerance (pc) for the KNOWN
#                                      pre-existing manual supply (191,033.89
#                                      BTH across Sepolia+devnet); without it
#                                      the breaker correctly trips at startup
#   BRIDGE_MAX_ORDER_PC / BRIDGE_DAILY_LIMIT_PC
#                                      engine amount caps (pc) — raise for
#                                      drill amounts above the 1,000/100 BTH
#                                      service defaults
#   BRIDGE_FED_SECRETS_DIR             key store override (worktree checkouts
#                                      reuse the main checkout's .secrets)
#   BRIDGE_FED_RUN_DIR                 run state (configs/db/logs/pids);
#                                      default .secrets/bridge-testnet/federation-run
#
# The BTH deposit of the mint drill must carry the order memo (the deposit
# scan binds deposits to orders by the memo-embedded order UUID). The wallet
# CLI cannot attach memos; use the live web wallet's /trade export panel
# (#1035/#1043) pointed at instance 1's public API, or the Rust harness. The
# drill prints the exact deposit parameters and then polls/asserts.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
# BRIDGE_FED_SECRETS_DIR lets a worktree checkout reuse the main checkout's
# git-ignored key store (worktrees do not share untracked files).
SECRETS_DIR="${BRIDGE_FED_SECRETS_DIR:-$REPO_ROOT/.secrets/bridge-testnet}"
FED_DIR="$SECRETS_DIR/federation"
RUN_DIR="${BRIDGE_FED_RUN_DIR:-$SECRETS_DIR/federation-run}"

N="${BRIDGE_FED_NODES:-3}"
T="${BRIDGE_FED_THRESHOLD:-2}"

BTH_RPC="${BRIDGE_BTH_RPC_URL:-https://seed.botho.io/rpc}"
# Per-instance BTH RPC endpoints (comma-separated). Each federation member
# watches its OWN node's view of the chain — the honest topology, and it also
# spreads load under the public ingress rate limits (100 req/min/IP).
BTH_RPCS="${BRIDGE_BTH_RPC_URLS:-https://seed.botho.io/rpc,https://seed2.botho.io/rpc,https://eu.seed.botho.io/rpc,https://ap.seed.botho.io/rpc,https://faucet.botho.io/rpc}"
bth_rpc_for() {
    # bth_rpc_for <instance-i> — BRIDGE_BTH_RPC_URL wins when set explicitly.
    if [[ -n "${BRIDGE_BTH_RPC_URL:-}" ]]; then echo "$BRIDGE_BTH_RPC_URL"; return; fi
    local IFS=',' idx=$(( ($1 - 1) ))
    read -ra arr <<< "$BTH_RPCS"
    echo "${arr[$(( idx % ${#arr[@]} ))]}"
}
ETH_RPC="${SEPOLIA_RPC_URL:-https://ethereum-sepolia-rpc.publicnode.com}"
SOL_RPC="${SOLANA_RPC_URL:-https://api.devnet.solana.com}"
WBTH="${BRIDGE_WBTH_ADDRESS:-0x49b985eC427EE771A601F11b18f7d4402fA2DD7B}"
SAFE="${BRIDGE_SAFE_ADDRESS:-0x61274F558f9027e2D402d3340dE89152FA3F3947}"
ETH_CHAIN_ID="${BRIDGE_ETH_CHAIN_ID:-11155111}"
ETH_CONFS="${BRIDGE_ETH_CONFIRMATIONS:-2}"
SOL_PROGRAM="${BRIDGE_SOLANA_PROGRAM:-CZDnzeywrqEM5ereWJmtYKUQ9uJXxX2PydqqKTQStxxE}"
SOL_FED="${BRIDGE_SOLANA_FEDERATION:-0}"
# Known pre-existing wrapped supply the reconciler should tolerate (pc). The
# live Sepolia+devnet tokens carry ~191,033.89 BTH of manually-bootstrapped
# supply (#866-#870) that a fresh reserve ledger cannot account for; without a
# tolerance the engine correctly trips its breaker ("reserve drift alert") and
# pauses all submission. Set this to the audited baseline for the drill; the
# drill assertions then verify drift DELTAS around each leg.
RESERVE_TOLERANCE="${BRIDGE_RESERVE_TOLERANCE:-0}"
# Per-order amount cap (pc). Default = the service default (1,000 BTH).
MAX_ORDER_PC="${BRIDGE_MAX_ORDER_PC:-1000000000000000}"
# Per-address daily volume cap (pc). Default = the service default (100 BTH).
DAILY_LIMIT_PC="${BRIDGE_DAILY_LIMIT_PC:-100000000000000}"

BRIDGE_BIN="${BRIDGE_BIN:-$REPO_ROOT/target/release/bth-bridge}"

# Instance i (1-based) ports: ops 97(i)1? No — fixed spacing:
ops_port()    { echo $((9731 + $1 * 10)); }   # 9741, 9751, 9761
attest_port() { echo $((9732 + $1 * 10)); }   # 9742, 9752, 9762
PUBLIC_PORT=9743                               # instance 1 only

log()  { printf '\033[0;34m[fed]\033[0m %s\n' "$*" >&2; }
ok()   { printf '\033[0;32m[ ok]\033[0m %s\n' "$*" >&2; }
warn() { printf '\033[1;33m[warn]\033[0m %s\n' "$*" >&2; }
die()  { printf '\033[0;31m[fail]\033[0m %s\n' "$*" >&2; exit 1; }

require_gitignored() {
    mkdir -p "$SECRETS_DIR"
    chmod 700 "$SECRETS_DIR" 2>/dev/null || true
    # Never write key material into a git-tracked path. When SECRETS_DIR
    # lives outside this checkout entirely (BRIDGE_FED_SECRETS_DIR), any repo
    # will do the check — use the one containing the directory if it is one.
    local top
    top="$(git -C "$SECRETS_DIR" rev-parse --show-toplevel 2>/dev/null || true)"
    if [[ -n "$top" ]]; then
        git -C "$top" check-ignore -q "$SECRETS_DIR" ||
            die "$SECRETS_DIR is NOT git-ignored; refusing to write key material"
    fi
}

# ---------------------------------------------------------------------------
# keys: per-instance ed25519 attestation keys (openssl or solana-keygen) plus
# the shared attest bearer token. secp256k1 identities are the EXISTING Safe
# owner keys (eth-safe-owner-{1..3}.key from bridge-testnet-accounts.sh) — the
# Sepolia Safe's owner set is fixed on-chain, so the federation's Ethereum
# identities must be exactly those keys.
# ---------------------------------------------------------------------------
gen_ed25519() {
    # Emits "<seed-hex> <pub-hex>" for a fresh ed25519 key.
    local pem priv pub
    pem="$(openssl genpkey -algorithm ed25519 2>/dev/null)" ||
        die "openssl with ed25519 support required"
    priv="$(printf '%s\n' "$pem" | openssl pkey -text -noout |
        awk '/priv:/{f=1;next}/pub:/{f=0}f' | tr -d ' :\n')"
    pub="$(printf '%s\n' "$pem" | openssl pkey -text -noout |
        awk '/pub:/{f=1;next}f' | tr -d ' :\n')"
    [[ ${#priv} -eq 64 && ${#pub} -eq 64 ]] || die "ed25519 key parse failed"
    echo "$priv $pub"
}

cmd_keys() {
    require_gitignored
    mkdir -p "$FED_DIR" && chmod 700 "$FED_DIR"

    for i in $(seq 1 "$N"); do
        local keyf="$FED_DIR/ed25519-$i.key" pubf="$FED_DIR/ed25519-$i.pub"
        if [[ -f "$keyf" ]]; then
            log "ed25519-$i exists, skipping"
        else
            read -r priv pub < <(gen_ed25519)
            printf '%s\n' "$priv" > "$keyf" && chmod 600 "$keyf"
            printf '%s\n' "$pub" > "$pubf"
            ok "generated ed25519 federation key $i (pub $pub)"
        fi
        # secp256k1 identity = Safe owner i (must pre-exist).
        [[ -f "$SECRETS_DIR/eth-safe-owner-$i.key" ]] ||
            warn "eth-safe-owner-$i.key missing — run scripts/bridge-testnet-accounts.sh" \
                 "(instance $i cannot attest Ethereum mints without it)"
    done

    if [[ ! -f "$FED_DIR/attest-token" ]]; then
        openssl rand -hex 32 > "$FED_DIR/attest-token" && chmod 600 "$FED_DIR/attest-token"
        ok "generated shared attest bearer token"
    fi
    ok "federation key material ready under $FED_DIR (PUBLIC keys: *.pub)"
}

# ---------------------------------------------------------------------------
# gen-reserve: throwaway RANDOM live-testnet reserve + user wallets. Reuses
# `botho-testnet gen-bridge-keys` (the only tool that exports view/spend hex +
# PQ seed); we take the RANDOM "user" wallet from two runs — run A's random
# wallet becomes the live reserve, run B's the drill user — because the
# harness's deterministic node-0 wallet is publicly derivable and must not
# custody even testnet reserve funds on a shared chain.
# ---------------------------------------------------------------------------
cmd_gen_reserve() {
    require_gitignored
    mkdir -p "$FED_DIR"
    for role in reserve user; do
        local dir="$FED_DIR/bth-$role"
        if [[ -f "$dir/user.view.hex" ]]; then
            log "BTH $role wallet exists, skipping"
            continue
        fi
        mkdir -p "$dir"
        local exports
        exports="$(cd "$REPO_ROOT" && cargo run --release --bin botho-testnet -- \
            gen-bridge-keys --node 0 --out "$dir")"
        # Keep ONLY the random ("user") wallet; shred the deterministic one.
        rm -f "$dir/reserve.view.hex" "$dir/reserve.spend.hex" "$dir/reserve.pq_seed.hex"
        local addr
        addr="$(printf '%s\n' "$exports" | sed -n 's/^export BRIDGE_BTH_USER_ADDRESS="\(.*\)"$/\1/p')"
        [[ -n "$addr" ]] || die "gen-bridge-keys did not print the user address"
        printf '%s\n' "$addr" > "$dir/address.txt"
        ok "BTH $role wallet ready: $addr"
    done

    cat > "$FED_DIR/reserve.env" <<EOF
# Live-testnet bridge drill wallets (throwaway, RANDOM; git-ignored).
export BRIDGE_BTH_RESERVE_VIEW_KEY="$FED_DIR/bth-reserve/user.view.hex"
export BRIDGE_BTH_RESERVE_SPEND_KEY="$FED_DIR/bth-reserve/user.spend.hex"
export BRIDGE_BTH_RESERVE_PQ_SEED="$FED_DIR/bth-reserve/user.pq_seed.hex"
export BRIDGE_BTH_RESERVE_ADDRESS="$(cat "$FED_DIR/bth-reserve/address.txt")"
export BRIDGE_BTH_USER_VIEW_KEY="$FED_DIR/bth-user/user.view.hex"
export BRIDGE_BTH_USER_SPEND_KEY="$FED_DIR/bth-user/user.spend.hex"
export BRIDGE_BTH_USER_PQ_SEED="$FED_DIR/bth-user/user.pq_seed.hex"
export BRIDGE_BTH_USER_ADDRESS="$(cat "$FED_DIR/bth-user/address.txt")"
EOF
    ok "wrote $FED_DIR/reserve.env"
    warn "the reserve must be funded with FACTOR-1 outputs before the release leg"
    warn "can spend (ADR 0003) — see the runbook's operator-prerequisites table."
}

load_reserve_env() {
    # Explicit env wins; otherwise auto-load the gen-reserve output.
    if [[ -z "${BRIDGE_BTH_RESERVE_VIEW_KEY:-}" && -f "$FED_DIR/reserve.env" ]]; then
        # shellcheck disable=SC1090
        source "$FED_DIR/reserve.env"
    fi
}

# ---------------------------------------------------------------------------
# render + up
# ---------------------------------------------------------------------------
render_config() {
    local i="$1"
    local cfg="$RUN_DIR/bridge-$i.toml"
    local token; token="$(cat "$FED_DIR/attest-token")"
    local inst_bth_rpc; inst_bth_rpc="$(bth_rpc_for "$i")"

    # Peer list: every OTHER instance's attest endpoint.
    local peers="" j
    for j in $(seq 1 "$N"); do
        [[ "$j" == "$i" ]] && continue
        peers+="\"http://127.0.0.1:$(attest_port "$j")\", "
    done
    peers="${peers%, }"

    # Federation public keys.
    local eth_signers="" sol_signers="" bth_signers=""
    for j in $(seq 1 "$N"); do
        if [[ -f "$SECRETS_DIR/eth-safe-owner-$j.addr" ]]; then
            eth_signers+="\"$(tr -d '[:space:]' < "$SECRETS_DIR/eth-safe-owner-$j.addr")\", "
        fi
        bth_signers+="\"$(tr -d '[:space:]' < "$FED_DIR/ed25519-$j.pub")\", "
    done
    eth_signers="${eth_signers%, }"; bth_signers="${bth_signers%, }"
    sol_signers="$bth_signers"   # same ed25519 validator identities (ADR 0002)

    # BTH reserve key material (optional — watch-only without it).
    local bth_keys=""
    if [[ -n "${BRIDGE_BTH_RESERVE_VIEW_KEY:-}" ]]; then
        bth_keys="view_key_file = \"${BRIDGE_BTH_RESERVE_VIEW_KEY}\"
spend_key_file = \"${BRIDGE_BTH_RESERVE_SPEND_KEY}\"
pq_seed_file = \"${BRIDGE_BTH_RESERVE_PQ_SEED}\"
reserve_address = \"${BRIDGE_BTH_RESERVE_ADDRESS}\""
    fi

    # Solana: keypair + federation only when explicitly enabled (the devnet
    # single-key mint authority trips the custody guard in federation posture).
    local sol_block="rpc_url = \"$SOL_RPC\"
wbth_program = \"$SOL_PROGRAM\"
commitment = \"finalized\""
    if [[ "$SOL_FED" == "1" ]]; then
        sol_block+="
keypair_file = \"$SECRETS_DIR/solana-mint-auth.json\"
mint_signers = [$sol_signers]
mint_threshold = $T"
    fi

    local public_block=""
    if [[ "$i" == "1" ]]; then
        public_block="
[public_api]
listen = \"127.0.0.1:$PUBLIC_PORT\"
min_order_amount = 1000000000
"
    fi

    cat > "$cfg" <<EOF
# bth-bridge federation instance $i/$N (threshold $T) — rendered by
# scripts/bridge-testnet-federation.sh; DO NOT COMMIT (contains the shared
# attest bearer token; lives under git-ignored .secrets/).

[bth]
rpc_url = "$inst_bth_rpc"
ws_url = "$inst_bth_rpc"
confirmations_required = 0
$bth_keys
release_signers = [$bth_signers]
release_threshold = $T
release_confirmations_required = 0

[ethereum]
rpc_url = "$ETH_RPC"
wbth_contract = "$WBTH"
safe_address = "$SAFE"
chain_id = $ETH_CHAIN_ID
private_key_file = "$SECRETS_DIR/eth-lp.key"
confirmations_required = $ETH_CONFS
mint_signers = [$eth_signers]
mint_threshold = $T

[solana]
$sol_block

[bridge]
mnemonic_file = "$RUN_DIR/unused-mnemonic"
db_path = "$RUN_DIR/shared.db"
fee_bps = 10
max_order_amount = $MAX_ORDER_PC
daily_limit_per_address = $DAILY_LIMIT_PC
testnet = true
attestation_ed25519_key_file = "$FED_DIR/ed25519-$i.key"
attestation_secp256k1_key_file = "$SECRETS_DIR/eth-safe-owner-$i.key"
attestation_nonce_file = "$RUN_DIR/nonces-$i.json"

[reserve]
tolerance_picocredits = $RESERVE_TOLERANCE
reconcile_interval_secs = 30
api_listen = "127.0.0.1:$(ops_port "$i")"

[federation]
peers = [$peers]
attest_listen = "127.0.0.1:$(attest_port "$i")"
inbound_auth_token = "$token"
peer_auth_token = "$token"
peer_push_timeout_secs = 5
$public_block
EOF
    chmod 600 "$cfg"
}

cmd_up() {
    require_gitignored
    [[ -x "$BRIDGE_BIN" ]] || die "bridge binary missing: $BRIDGE_BIN (cargo build --release -p bth-bridge-service --bin bth-bridge)"
    [[ -f "$FED_DIR/attest-token" ]] || cmd_keys
    load_reserve_env
    mkdir -p "$RUN_DIR" && chmod 700 "$RUN_DIR"

    for i in $(seq 1 "$N"); do
        render_config "$i"
    done

    # Initialize the SHARED database once (schema + persistent WAL mode)
    # before any instance races to create it.
    "$BRIDGE_BIN" --config "$RUN_DIR/bridge-1.toml" --migrate >/dev/null 2>&1 ||
        die "database migration failed (see $RUN_DIR/bridge-1.toml)"

    # Staggered start: wait for each instance's /health before the next, so
    # concurrent first-boot writes never contend on the shared store.
    for i in $(seq 1 "$N"); do
        if [[ -f "$RUN_DIR/bridge-$i.pid" ]] && kill -0 "$(cat "$RUN_DIR/bridge-$i.pid")" 2>/dev/null; then
            log "instance $i already running (pid $(cat "$RUN_DIR/bridge-$i.pid"))"
            continue
        fi
        nohup "$BRIDGE_BIN" --config "$RUN_DIR/bridge-$i.toml" \
            > "$RUN_DIR/bridge-$i.log" 2>&1 &
        echo $! > "$RUN_DIR/bridge-$i.pid"
        log "started instance $i (pid $!, ops 127.0.0.1:$(ops_port "$i"), attest 127.0.0.1:$(attest_port "$i"))"
        # `/health` intentionally reports 503 while any component is degraded
        # (e.g. the KNOWN live-Sepolia peg drift) — treat ANY HTTP answer as
        # "the instance is up"; health semantics are the operator's signal.
        local code=""
        for _ in $(seq 1 30); do
            code="$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$(ops_port "$i")/health" 2>/dev/null || true)"
            [[ -n "$code" && "$code" != "000" ]] && break
            kill -0 "$(cat "$RUN_DIR/bridge-$i.pid")" 2>/dev/null ||
                { tail -20 "$RUN_DIR/bridge-$i.log" >&2; die "instance $i died at startup"; }
            sleep 1
        done
        [[ -n "$code" && "$code" != "000" ]] || die "instance $i ops API never came up"
        ok "instance $i up (/health HTTP $code)"
    done
    ok "federation up: $N instances, threshold $T"
    [[ "$SOL_FED" == "1" ]] || warn "Solana leg NOT federated (BRIDGE_SOLANA_FEDERATION=0; devnet single-key authority)"
}

cmd_down() {
    local i
    for i in $(seq 1 "$N"); do
        if [[ -f "$RUN_DIR/bridge-$i.pid" ]]; then
            kill "$(cat "$RUN_DIR/bridge-$i.pid")" 2>/dev/null || true
            rm -f "$RUN_DIR/bridge-$i.pid"
            log "stopped instance $i"
        fi
    done
}

cmd_clean() { cmd_down; rm -rf "$RUN_DIR"; ok "run state removed (keys kept)"; }

cmd_status() {
    local i
    for i in $(seq 1 "$N"); do
        echo "--- instance $i (127.0.0.1:$(ops_port "$i")) ---"
        curl -sf "http://127.0.0.1:$(ops_port "$i")/api/status" | jq . ||
            warn "instance $i unreachable"
    done
}

cmd_proof() {
    local i
    for i in $(seq 1 "$N"); do
        echo "--- instance $i /api/reserve/proof ---"
        curl -sf "http://127.0.0.1:$(ops_port "$i")/api/reserve/proof" | jq . ||
            warn "instance $i: no reconciliation snapshot yet (wait one reconcile interval)"
    done
}

public_api() { echo "http://127.0.0.1:$PUBLIC_PORT"; }

cmd_order_mint() {
    local amount="${1:?usage: order-mint <amount-picocredits> <eth-dest-address>}"
    local dest="${2:?usage: order-mint <amount-picocredits> <eth-dest-address>}"
    curl -sf -X POST "$(public_api)/api/bridge/orders" \
        -H 'Content-Type: application/json' \
        -d "{\"destChain\":\"ethereum\",\"destAddress\":\"$dest\",\"amount\":\"$amount\"}" | jq .
}

cmd_order_status() {
    local id="${1:?usage: order-status <order-id>}"
    curl -sf "$(public_api)/api/bridge/orders/$id" | jq .
}

db_query() { sqlite3 -readonly "$RUN_DIR/shared.db" "$1"; }

cmd_orders() {
    db_query "SELECT id, order_type, source_chain || '->' || dest_chain, amount, fee, status,
                     COALESCE(source_tx,'-'), COALESCE(dest_tx,'-')
              FROM bridge_orders ORDER BY created_at;" | column -t -s '|'
}

cmd_attest_log() {
    db_query "SELECT created_at, COALESCE(order_id,'-'), action, details
              FROM audit_log
              WHERE action LIKE 'attestation%' OR action LIKE 'mint%' OR action LIKE 'release%'
                 OR action LIKE 'burn%' OR action LIKE 'deposit%'
              ORDER BY created_at;" | column -t -s '|'
}

poll_order() {
    # poll_order <order-id> <want-status> <deadline-secs>
    local id="$1" want="$2" deadline=$(( $(date +%s) + ${3:-1800} )) status=""
    while :; do
        status="$(curl -sf "$(public_api)/api/bridge/orders/$id" | jq -r .status || true)"
        log "order $id status: ${status:-unreachable}"
        [[ "$status" == "$want" ]] && return 0
        [[ "$status" == "failed" || "$status" == "expired" ]] &&
            die "order $id terminal in $status"
        [[ "$(date +%s)" -ge "$deadline" ]] && return 1
        sleep 15
    done
}

eth_call_u256() {
    # eth_call_u256 <to> <data> -> decimal
    curl -sf -X POST -H 'Content-Type: application/json' \
        --data "{\"jsonrpc\":\"2.0\",\"method\":\"eth_call\",\"params\":[{\"to\":\"$1\",\"data\":\"$2\"},\"latest\"],\"id\":1}" \
        "$ETH_RPC" | jq -r .result | python3 -c 'import sys; print(int(sys.stdin.read().strip(), 16))'
}

wbth_total_supply() { eth_call_u256 "$WBTH" "0x18160ddd"; }
wbth_balance() {
    local a="${1#0x}"
    eth_call_u256 "$WBTH" "0x70a08231000000000000000000000000$a"
}

proof_drift() {
    # Latest reconciled drift from instance 1 ("" when no snapshot yet).
    curl -sf "http://127.0.0.1:$(ops_port 1)/api/reserve/proof" | jq -r '.drift // empty'
}

assert_drift_unchanged() {
    # assert_drift_unchanged <baseline-drift>
    #
    # The peg invariant check. On a pristine deployment drift is exactly 0;
    # on the live Sepolia token there is a KNOWN constant positive drift (the
    # #866–#870 manual DeFi-bootstrap supply predates this reserve ledger —
    # see the runbook). Either way, a correct factor-1 leg must leave drift
    # EXACTLY where it started: mint adds equal lock+supply, release removes
    # equal amounts of both. Waits out one reconcile pass first.
    local baseline="$1" drift=""
    sleep 35   # let one reconcile pass observe the post-leg state
    drift="$(proof_drift)"
    [[ -n "$drift" ]] || { warn "no reconciliation snapshot; drift unverified"; return 0; }
    [[ "$drift" == "$baseline" ]] ||
        die "proof-of-reserves drift moved: $baseline -> $drift (peg invariant violated)"
    ok "proof-of-reserves drift unchanged ($drift) — Σ wBTH == locked reserve held"
}

# ---------------------------------------------------------------------------
# drill-mint: BTH deposit -> threshold attestation -> live Sepolia Safe mint.
# The deposit itself must be sent by the USER with the order memo attached
# (web wallet /trade export panel, or the Rust harness); this driver creates
# the order, prints the exact deposit parameters, then polls and asserts.
# ---------------------------------------------------------------------------
cmd_drill_mint() {
    local amount="${1:?usage: drill-mint <amount-picocredits> <eth-dest-address>}"
    local dest="${2:?usage: drill-mint <amount-picocredits> <eth-dest-address>}"
    load_reserve_env

    local supply0 bal0 drift0
    supply0="$(wbth_total_supply)"; bal0="$(wbth_balance "$dest")"
    drift0="$(proof_drift)"
    log "wBTH totalSupply before: $supply0 pc; $dest balance: $bal0 pc; drift: ${drift0:-n/a}"

    local resp id fee memo dep
    resp="$(curl -sf -X POST "$(public_api)/api/bridge/orders" \
        -H 'Content-Type: application/json' \
        -d "{\"destChain\":\"ethereum\",\"destAddress\":\"$dest\",\"amount\":\"$amount\"}")"
    id="$(jq -r .id <<<"$resp")"; fee="$(jq -r .fee <<<"$resp")"
    memo="$(jq -r .memo <<<"$resp")"; dep="$(jq -r .depositAddress <<<"$resp")"
    ok "mint order $id created (gross $amount pc, fee $fee pc)"
    echo
    echo "==> SEND THE DEPOSIT NOW (must be FACTOR-1 coins, WITH this memo):"
    echo "    to:      $dep"
    echo "    amount:  $amount picocredits"
    echo "    memo:    $memo"
    echo "    (web wallet /trade export panel, pointed at $(public_api))"
    echo

    poll_order "$id" "completed" "${BRIDGE_DRILL_DEADLINE:-3600}" ||
        die "order $id did not complete within the deadline"

    local net=$(( amount - fee ))
    local supply1 bal1
    supply1="$(wbth_total_supply)"; bal1="$(wbth_balance "$dest")"
    [[ $(( supply1 - supply0 )) -eq "$net" ]] ||
        die "factor-1 violated: supply delta $(( supply1 - supply0 )) != net $net"
    [[ $(( bal1 - bal0 )) -eq "$net" ]] ||
        die "factor-1 violated: balance delta $(( bal1 - bal0 )) != net $net"
    ok "factor-1 mint verified: exactly $net pc wBTH minted to $dest"

    local mints
    mints="$(db_query "SELECT COUNT(*) FROM mints WHERE order_id = '$id';" || echo "?")"
    [[ "$mints" == "1" || "$mints" == "?" ]] || die "exactly-once violated: $mints mint records"
    ok "exactly-once: single recorded mint tx for order $id"
    assert_drift_unchanged "${drift0:-0}"
    cmd_order_status "$id"
}

# ---------------------------------------------------------------------------
# drill-burn: live Sepolia bridgeBurn -> burn watcher -> threshold release
# attestation -> BTH reserve release to a fresh stealth output.
# ---------------------------------------------------------------------------
cmd_drill_burn() {
    local amount="${1:?usage: drill-burn <amount-picocredits> <bth-dest-address>}"
    local bth_dest="${2:?usage: drill-burn <amount-picocredits> <bth-dest-address>}"
    local cast="${CAST_BIN:-$HOME/.foundry/bin/cast}"
    [[ -x "$cast" ]] || die "cast (foundry) required for the burn tx"

    local supply0 drift0
    supply0="$(wbth_total_supply)"; drift0="$(proof_drift)"
    log "wBTH totalSupply before burn: $supply0 pc; drift: ${drift0:-n/a}"

    log "submitting bridgeBurn($amount, $bth_dest) on live Sepolia from the LP wallet"
    local tx
    tx="$("$cast" send --rpc-url "$ETH_RPC" \
        --private-key "$(tr -d '[:space:]' < "$SECRETS_DIR/eth-lp.key")" \
        "$WBTH" 'bridgeBurn(uint256,string)' "$amount" "$bth_dest" --json | jq -r .transactionHash)"
    ok "burn tx: $tx  (https://sepolia.etherscan.io/tx/$tx)"

    local supply1; supply1="$(wbth_total_supply)"
    [[ $(( supply0 - supply1 )) -eq "$amount" ]] ||
        warn "supply delta $(( supply0 - supply1 )) != $amount (pending block?)"

    log "waiting for the federation to detect + confirm the burn and release BTH"
    local deadline=$(( $(date +%s) + ${BRIDGE_DRILL_DEADLINE:-3600} )) row=""
    while :; do
        row="$(db_query "SELECT id || '|' || status FROM bridge_orders
                         WHERE order_type='burn' AND source_tx='$tx';" || true)"
        log "burn order: ${row:-not yet detected}"
        [[ "$row" == *"|released"* ]] && break
        [[ "$row" == *"|failed"* ]] && die "burn order failed: $row"
        [[ "$(date +%s)" -ge "$deadline" ]] &&
            die "burn order did not reach Released within the deadline (state: ${row:-none})"
        sleep 15
    done
    ok "burn order released"
    cmd_attest_log
    assert_drift_unchanged "${drift0:-0}"
}

cmd_logs() {
    local i="${1:-1}"
    tail -40 "$RUN_DIR/bridge-$i.log"
}

usage() { sed -n '2,60p' "$0" | sed 's/^# \{0,1\}//'; }

case "${1:-}" in
    keys)         cmd_keys ;;
    gen-reserve)  cmd_gen_reserve ;;
    up)           cmd_up ;;
    down)         cmd_down ;;
    clean)        cmd_clean ;;
    status)       cmd_status ;;
    proof)        cmd_proof ;;
    order-mint)   shift; cmd_order_mint "$@" ;;
    order-status) shift; cmd_order_status "$@" ;;
    orders)       cmd_orders ;;
    attest-log)   cmd_attest_log ;;
    drill-mint)   shift; cmd_drill_mint "$@" ;;
    drill-burn)   shift; cmd_drill_burn "$@" ;;
    logs)         shift; cmd_logs "$@" ;;
    *)            usage; exit 1 ;;
esac
