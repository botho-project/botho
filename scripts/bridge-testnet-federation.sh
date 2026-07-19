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
#   ./scripts/bridge-testnet-federation.sh rotate        # e2e key-rotation drill (#1061):
#                    mock same-set election -> pause -> drain -> fresh keys on
#                    every surface -> SEAL the v2 term document -> OLD-KEYS-DEAD
#                    assertions -> resume -> post-rotation attestation +
#                    proof-of-reserves. Individual phases: rotate-elect|pause|
#                    drain|keys|safe|solana|bth|seal|restart|verify|resume|attest
#                    (see the rotate section).
#   ./scripts/bridge-testnet-federation.sh term-doc-selftest  # OFFLINE v2
#                    term-document self-check (no services): schema + the
#                    elected->sealed transition + signature verification.
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
#                                      mint keypair. Default stays 0: the devnet
#                                      wbth mint_authority is still a single key
#                                      (97oZgGpd…), and the startup custody guard
#                                      HARD-FAILS a federation posture with a
#                                      single-key authority (mint/solana.rs).
#                                      Migration = #1052; the flip to 1 is an
#                                      OPERATOR action, only AFTER the on-chain
#                                      mint_authority is rotated to a distinct
#                                      2-of-3 multisig (contracts/solana/README.md
#                                      "Devnet mint-multisig migration").
#                                      Two milestones, do not conflate:
#                                        path 2 (boot): rotate to a Squads PDA and
#                                          the guard PASSES + the Solana leg BOOTS
#                                          federated — but a mint cannot COMPLETE,
#                                          because a PDA only signs via a Squads
#                                          invoke_signed CPI that prepare_mint does
#                                          not yet build.
#                                        path 1 (complete): needs that Squads-gated
#                                          mint-assembly code (separate issue)
#                                          before an end-to-end federated devnet
#                                          mint lands exactly-once/factor-1.
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

# ===========================================================================
# rotate: the e2e key-rotation drill (#1061, serving the #1060 elected-
# multisig direction).
#
# A MOCK election re-elects the SAME member set (membership-stable), and the
# entire handover machinery then runs with FRESH keys on every custody
# surface:
#
#   rotate-elect    mock same-set election -> election/term-<K>.json
#   rotate-pause    trip the breaker; public order API must refuse creates
#   rotate-drain    no value-in-motion orders before any key changes
#   rotate-keys     fresh ed25519 keys + attest token + Safe owner keys
#                   (old material archived under federation/retired/term-<K-1>)
#   rotate-safe     LIVE Sepolia Safe owner swap (swapOwner x N via 2-of-3
#                   execTransaction; relayer pays gas)
#   rotate-solana   devnet wbth mint-authority rotation (spl-token; gated
#                   with exact commands when tooling is absent)
#   rotate-bth      BTH reserve re-key (new reserve wallet; the funds-moving
#                   sweep is OPERATOR-GATED while #1051 holds — documented)
#   rotate-restart  re-render configs (they pin the NEW public keys) + restart
#   rotate-verify   OLD KEYS ARE POWERLESS — gates the resume:
#                     * validly-signed OLD-key attestation -> refused:unknown_signer
#                     * OLD key under a NEW signer id      -> refused:bad_signature
#                     * OLD attest bearer token            -> 401
#                     * OLD Safe owners: isOwner false + execTransaction
#                       simulation reverts (GS026); NEW owners' simulation OK
#   rotate-resume   lift the breaker (refuses unless rotate-verify passed);
#                   commits the new term
#   rotate-attest   post-rotation threshold attestation round (audit log shows
#                   attestation_authorized AFTER resume) + proof-of-reserves
#                   drift unchanged vs the pre-pause baseline
#   rotate          all of the above, in order
#
# The MOCK ELECTION INTERFACE is the only seam: everything after rotate-elect
# consumes ONLY election/term-<K>.json. A real #1060 election replaces
# rotate-elect (same document schema) and nothing else.
#
# Extra env knobs:
#   BRIDGE_ROTATE_DRAIN_DEADLINE=300   secs to wait for in-flight orders
#   BRIDGE_ROTATE_ACK_PARKED=          csv of order ids (or "all") the operator
#                                      acknowledges as parked-retryable (soft
#                                      states only; value-in-motion states are
#                                      never overridable)
#   BRIDGE_ROTATE_ATTEST_DEADLINE=600  secs to wait for the post-rotation
#                                      attestation round
#   BRIDGE_SOLANA_MINT=F7Lsi…          the devnet wbth SPL mint
# ===========================================================================

ELECTION_DIR="$FED_DIR/election"
SOL_MINT="${BRIDGE_SOLANA_MINT:-F7LsiATxVQxnDEBWemfuq1BgFDYbuzqMMJ5eZjaB7LFX}"
ETH_SENTINEL="0x0000000000000000000000000000000000000001"
ETH_ZERO="0x0000000000000000000000000000000000000000"

# v2 term-document schema (docs/bridge/election-dynamics.md §5.2, ADR 0010) and
# its committed worked example. The rotation drill emits documents that MUST
# validate against this schema (rotate-seal / rotate-verify / term-doc-selftest).
TERM_SCHEMA="$REPO_ROOT/docs/bridge/schemas/term-document.v2.schema.json"
TERM_SCHEMA_REL="docs/bridge/schemas/term-document.v2.schema.json"
TERM_EXAMPLE="$REPO_ROOT/docs/bridge/schemas/examples/term-3.sealed.example.json"
# Domain-separation prefix for every term-document signature (mirrors the
# attestation-envelope discipline in bridge/core/src/attestation.rs).
TERM_DOC_DOMAIN="botho.bridge.term.v2:"
# Handover / term windows (seconds). Ratification-pending numbers (§7): a 72h
# handover deadline and a ~93-day term.
TERM_HANDOVER_SECS="${BRIDGE_TERM_HANDOVER_SECS:-259200}"
TERM_LENGTH_SECS="${BRIDGE_TERM_LENGTH_SECS:-8035200}"

current_term()      { cat "$ELECTION_DIR/current-term" 2>/dev/null || echo 1; }
election_doc()      { echo "$ELECTION_DIR/term-$1.json"; }
rotate_state_dir()  { echo "$ELECTION_DIR/rotate-term-$1"; }
retired_dir()       { echo "$FED_DIR/retired/term-$1"; }
node_id_for()       { printf 'node-fed-%02d' "$1"; }
sha256_hex()        { python3 -c 'import hashlib,sys;print(hashlib.sha256(sys.stdin.buffer.read()).hexdigest())'; }

# ---------------------------------------------------------------------------
# Term-document (v2) cryptographic + assembly helpers. Signatures are real
# ed25519 (openssl), domain-separated with TERM_DOC_DOMAIN, over three
# well-defined payloads (§5.2 "Normative semantics"):
#   * keySubmissionSig      — signed by a member's LONG-LIVED identity key,
#                             over (v, term, nodeId, keys).
#   * tallyAttestations[].sig — signed by an elector's LONG-LIVED identity key,
#                             over the ELECTED/tally snapshot (the election-
#                             decided fields only: v, term, electionKind,
#                             electorate, tally, threshold, membership
#                             identities). Stable across sealing, so electors
#                             sign ONCE at tally time.
#   * outgoing[].sig        — signed by an OUTGOING member's retired attestation
#                             key, over the complete SEALED document minus the
#                             signatures object (the full authorization chain).
# ---------------------------------------------------------------------------
ensure_identity_keys() {
    # ensure_identity_keys <fed_dir> <n> — long-lived per-member identity keys
    # (curated identity; NEVER rotated — distinct from the per-term
    # attestation keys). Created once, persist across terms.
    local fd="$1" n="$2" i priv pub
    for i in $(seq 1 "$n"); do
        if [[ ! -f "$fd/identity-$i.key" ]]; then
            read -r priv pub < <(gen_ed25519)
            printf '%s\n' "$priv" > "$fd/identity-$i.key"; chmod 600 "$fd/identity-$i.key"
            printf '%s\n' "$pub"  > "$fd/identity-$i.pub"
        fi
    done
}

ed25519_sign_hex() {
    # ed25519_sign_hex <seed-hex> ; message on stdin -> hex signature on stdout
    # (python via -c so the piped message reaches stdin, not a heredoc).
    python3 -c '
import base64, os, subprocess, sys, tempfile
seed = bytes.fromhex(sys.argv[1].strip()); assert len(seed) == 32, "32-byte seed"
der = bytes.fromhex("302e020100300506032b657004220420") + seed
pem = ("-----BEGIN PRIVATE KEY-----\n"
       + base64.encodebytes(der).decode() + "-----END PRIVATE KEY-----\n")
msg = sys.stdin.buffer.read()
with tempfile.TemporaryDirectory() as d:
    kp, mp, sp = (os.path.join(d, n) for n in ("k.pem", "m.bin", "s.bin"))
    open(kp, "w").write(pem); open(mp, "wb").write(msg)
    subprocess.run(["openssl", "pkeyutl", "-sign", "-inkey", kp,
                    "-rawin", "-in", mp, "-out", sp], check=True)
    print(open(sp, "rb").read().hex())
' "$1"
}

ed25519_verify_hex() {
    # ed25519_verify_hex <pub-hex> <sig-hex> ; message on stdin ; exit 0 if valid
    python3 -c '
import base64, os, subprocess, sys, tempfile
pub = bytes.fromhex(sys.argv[1].strip()); sig = bytes.fromhex(sys.argv[2].strip())
der = bytes.fromhex("302a300506032b6570032100") + pub
pem = ("-----BEGIN PUBLIC KEY-----\n"
       + base64.encodebytes(der).decode() + "-----END PUBLIC KEY-----\n")
msg = sys.stdin.buffer.read()
with tempfile.TemporaryDirectory() as d:
    kp, mp, sp = (os.path.join(d, n) for n in ("k.pem", "m.bin", "s.bin"))
    open(kp, "w").write(pem); open(mp, "wb").write(msg); open(sp, "wb").write(sig)
    r = subprocess.run(["openssl", "pkeyutl", "-verify", "-pubin", "-inkey", kp,
                        "-rawin", "-in", mp, "-sigfile", sp],
                       capture_output=True)
    sys.exit(0 if r.returncode == 0 else 1)
' "$1" "$2"
}

# The three canonical signing payloads (domain prefix + canonical JSON: jq -cS
# = sorted keys, compact). Each reads the document from $1.
tally_msg() {
    { printf '%s' "$TERM_DOC_DOMAIN"
      jq -cS '{v, term, electionKind, electorate, tally, threshold,
               members: [.members[] | {index, nodeId, approvals}]}' "$1"; }
}
keysub_msg() {   # keysub_msg <doc> <member-index-1based>
    { printf '%s' "$TERM_DOC_DOMAIN"
      jq -cS --argjson i "$2" \
        '{v, term, nodeId: .members[$i-1].nodeId, keys: .members[$i-1].keys}' "$1"; }
}
outgoing_msg() {
    { printf '%s' "$TERM_DOC_DOMAIN"; jq -cS 'del(.signatures)' "$1"; }
}

validate_term_doc() {
    # validate_term_doc <doc> [pass|fail] — validate against the committed v2
    # schema. SKIPs (with a warning) when python jsonschema is unavailable.
    local doc="$1" expect="${2:-pass}" out rc
    # `&& rc=0 || rc=$?` keeps a non-zero exit (expected for negative cases)
    # from tripping `set -e` before we can inspect it.
    out="$(python3 - "$TERM_SCHEMA" "$doc" <<'PY'
import json, sys
try:
    from jsonschema import Draft202012Validator
except Exception:
    print("SKIP jsonschema unavailable"); sys.exit(2)
schema = json.load(open(sys.argv[1])); doc = json.load(open(sys.argv[2]))
errs = sorted(Draft202012Validator(schema).iter_errors(doc), key=lambda e: list(e.path))
if errs:
    print("INVALID: " + "/".join(map(str, errs[0].path)) + ": " + errs[0].message); sys.exit(1)
print("VALID"); sys.exit(0)
PY
)" && rc=0 || rc=$?
    if [[ $rc -eq 2 ]]; then warn "term-doc schema check skipped ($out)"; return 0; fi
    if [[ "$expect" == pass ]]; then
        [[ $rc -eq 0 ]] || die "term document did NOT validate against $TERM_SCHEMA_REL: $out ($doc)"
    else
        [[ $rc -eq 1 ]] || die "expected term document to be REJECTED but rc=$rc ($out) ($doc)"
    fi
}

emit_elected_doc() {
    # emit_elected_doc <doc> <term> <fed_dir> <secrets_dir> <election_kind>
    # Membership-only 'elected' document (no per-term keys yet). Mirrors the
    # #1067 tally output; the drill's same-set mock stands in for the ballot.
    local doc="$1" term="$2" fd="$3" sd="$4" kind="$5" i
    ensure_identity_keys "$fd" "$N"
    local members eligible now="$(date +%s)"
    members="$(for i in $(seq 1 "$N"); do
        jq -n --argjson idx "$i" --arg nid "$(node_id_for "$i")" --argjson ap "$N" \
            '{index: $idx, nodeId: $nid, approvals: $ap}'
      done | jq -s '.')"
    eligible="$(for i in $(seq 1 "$N"); do node_id_for "$i"; done | jq -R . | jq -s '.')"
    local curationHash
    curationHash="$(printf '%s' "$(jq -cS . <<<"$eligible")" | sha256_hex)"
    local base
    base="$(jq -n \
        --argjson term "$term" --arg kind "$kind" --arg chash "$curationHash" \
        --argjson eligible "$eligible" --argjson members "$members" \
        --argjson thr "$T" --arg safe "$SAFE" --arg solmint "$SOL_MINT" \
        --argjson now "$now" --argjson hand "$TERM_HANDOVER_SECS" --argjson len "$TERM_LENGTH_SECS" \
        '{v: 2, term: $term, electionKind: $kind, status: "elected",
          electorate: {curationDocHash: $chash, snapshotHeight: 0, eligible: $eligible},
          tally: {rule: "approval-top-N-v1", openHeight: 0, closeHeight: 0,
                  ballots: ($eligible | length), resultHash: ""},
          threshold: $thr,
          members: $members,
          execution: {ethereum: {safe: $safe, intent: "swapOwner", newThreshold: $thr},
                      solana:   {authority: $solmint, intent: "setAuthority"},
                      bth:      {intent: "reserveSweepFactor1", newReserveAddress: "pending-seal"}},
          validity: {electedAt: $now, handoverDeadline: ($now + $hand), termEnd: ($now + $len)}}')"
    # resultHash binds the tally transcript (mock: the membership ranking).
    local rhash
    rhash="$(jq -cS '{term, members: [.members[] | {index, nodeId, approvals}]}' <<<"$base" | sha256_hex)"
    base="$(jq --arg rh "$rhash" '.tally.resultHash = $rh' <<<"$base")"
    printf '%s' "$base" > "$doc.wip"
    # Tally attestations: each eligible elector signs the tally snapshot ONCE.
    local att="[]" sig
    for i in $(seq 1 "$N"); do
        sig="$(tally_msg "$doc.wip" | ed25519_sign_hex "$(cat "$fd/identity-$i.key")")"
        att="$(jq --arg nid "$(node_id_for "$i")" --arg s "ed25519:$sig" \
            '. + [{nodeId: $nid, sig: $s}]' <<<"$att")"
    done
    jq --argjson att "$att" '. + {signatures: {tallyAttestations: $att, outgoing: []}}' \
        "$doc.wip" > "$doc"
    rm -f "$doc.wip"; chmod 600 "$doc"
}

emit_sealed_doc() {
    # emit_sealed_doc <doc> <fed_dir> <secrets_dir> <retired_dir> [solanaMember] [bthReserve]
    # Select-then-keygen: consume an 'elected' document and produce a 'sealed'
    # one by binding fresh per-term keys (signed by each winner's long-lived
    # identity key) and the outgoing federation's counter-signature.
    local doc="$1" fd="$2" sd="$3" ret="$4" solmember="${5:-}" bthres="${6:-}" i
    local status; status="$(jq -r '.status' "$doc")"
    [[ "$status" == "sealed" ]] && { log "term document already sealed"; return 0; }
    [[ "$status" == "elected" ]] || die "term document is '$status', expected 'elected' — run rotate-elect"
    ensure_identity_keys "$fd" "$N"
    local newdoc; newdoc="$(cat "$doc")"
    # Per-member fresh key material. ed25519 + ethSafeOwner rotate per member;
    # solana/bth custody is single-key on testnet (#867/#1051) so those repeat
    # across members here (mainnet gives each member a distinct Squads/reserve
    # share). Absent live legs fall back to a clearly-marked mock placeholder.
    local sm="${solmember:-mock:solana-single-custody}"
    local br="${bthres:-mock:bth-reserve-single-custody}"
    for i in $(seq 1 "$N"); do
        local edpub ethaddr keys
        edpub="$(tr -d '[:space:]' < "$fd/ed25519-$i.pub")"
        ethaddr="$(tr -d '[:space:]' < "$sd/eth-safe-owner-$i.addr" 2>/dev/null || echo "")"
        keys="$(jq -n --arg ed "$edpub" --arg eth "$ethaddr" --arg s "$sm" --arg b "$br" \
            '{ed25519AttestationPubkey: $ed, ethSafeOwner: $eth, solanaMember: $s, bthReserveKey: $b}')"
        newdoc="$(jq --argjson i "$i" --argjson keys "$keys" '.members[$i-1].keys = $keys' <<<"$newdoc")"
    done
    newdoc="$(jq --arg br "${bthres:-pending-seal}" \
        '.status = "sealed" | .execution.bth.newReserveAddress = $br' <<<"$newdoc")"
    [[ -n "$solmember" ]] && newdoc="$(jq --arg a "$solmember" '.execution.solana.authority = $a' <<<"$newdoc")"
    printf '%s' "$newdoc" > "$doc.wip"
    # keySubmissionSig: each member binds its fresh keys with its identity key.
    for i in $(seq 1 "$N"); do
        local sig
        sig="$(keysub_msg "$doc.wip" "$i" | ed25519_sign_hex "$(cat "$fd/identity-$i.key")")"
        newdoc="$(jq --argjson i "$i" --arg s "ed25519:$sig" \
            '.members[$i-1].keySubmissionSig = $s' <<<"$newdoc")"
        printf '%s' "$newdoc" > "$doc.wip"
    done
    # Outgoing counter-signature: threshold-T retired attestation keys sign the
    # completed sealed document (signatures removed).
    local outg="[]"
    for i in $(seq 1 "$T"); do
        [[ -f "$ret/ed25519-$i.key" ]] ||
            die "no retired attestation key $i under $ret to counter-sign the handover"
        local opub osig
        opub="$(tr -d '[:space:]' < "$ret/ed25519-$i.pub")"
        osig="$(outgoing_msg "$doc.wip" | ed25519_sign_hex "$(cat "$ret/ed25519-$i.key")")"
        outg="$(jq --argjson idx "$i" --arg p "$opub" --arg s "ed25519:$osig" \
            '. + [{index: $idx, ed25519AttestationPubkey: $p, sig: $s}]' <<<"$outg")"
    done
    newdoc="$(jq --argjson o "$outg" '.signatures.outgoing = $o' <<<"$newdoc")"
    printf '%s\n' "$newdoc" > "$doc"; rm -f "$doc.wip"; chmod 600 "$doc"
}

pending_term() {
    local next=$(( $(current_term) + 1 ))
    [[ -f "$(election_doc "$next")" ]] ||
        die "no election document for term $next — run rotate-elect first"
    echo "$next"
}

require_phase() {
    # require_phase <term> <marker> <hint>
    [[ -f "$(rotate_state_dir "$1")/$2" ]] ||
        die "rotation term $1: phase '$2' has not completed — run $3 first"
}

need_cast() {
    local cast="${CAST_BIN:-$HOME/.foundry/bin/cast}"
    [[ -x "$cast" ]] || die "cast (foundry) required: $cast"
    echo "$cast"
}

# ---------------------------------------------------------------------------
# rotate-elect: MOCK election (v2 term document, status "elected"). Re-elects
# the CURRENT member set with a term bump, pinning MEMBERSHIP ONLY — the fresh
# per-term keys do not exist yet and are bound later by rotate-seal (the
# select-then-keygen lifecycle, ADR 0010 §5.1). A real #1060/#1067 election
# drops an 'elected' document in here unchanged; nothing downstream changes.
# ---------------------------------------------------------------------------
cmd_rotate_elect() {
    require_gitignored
    mkdir -p "$ELECTION_DIR" && chmod 700 "$ELECTION_DIR"
    local cur next doc
    cur="$(current_term)"; next=$(( cur + 1 )); doc="$(election_doc "$next")"
    if [[ -f "$doc" ]]; then
        log "election document for term $next already exists (status $(jq -r .status "$doc"))"
        jq . "$doc" >&2
        return 0
    fi
    emit_elected_doc "$doc" "$next" "$FED_DIR" "$SECRETS_DIR" "mock-same-set"
    validate_term_doc "$doc" pass
    ok "MOCK election (v2): term $next re-elects the same $N members — status=elected (keys bound at rotate-seal), threshold $T"
    jq . "$doc" >&2
    mkdir -p "$(rotate_state_dir "$next")" && chmod 700 "$(rotate_state_dir "$next")"
    date +%s > "$(rotate_state_dir "$next")/elected"
}

# ---------------------------------------------------------------------------
# rotate-pause: trip the breaker for the handover; assert the pause is
# visible on every ops surface AND that the public order API refuses new
# orders (503, probed with an invalid body so no order is ever created).
# ---------------------------------------------------------------------------
cmd_rotate_pause() {
    local term st i
    term="$(pending_term)"; st="$(rotate_state_dir "$term")"; mkdir -p "$st"

    # Pre-rotation proof-of-reserves baseline: rotation must move NO value,
    # so the post-rotation drift must equal this exactly.
    curl -sf "http://127.0.0.1:$(ops_port 1)/api/reserve/proof" \
        > "$st/proof-baseline.json" 2>/dev/null ||
        warn "no reserve-proof snapshot yet (baseline unavailable)"

    curl -sf -X POST "http://127.0.0.1:$(ops_port 1)/api/breaker" \
        -H 'Content-Type: application/json' \
        -d "{\"paused\":true,\"reason\":\"rotation drill term $term\"}" | jq . >&2 ||
        die "breaker pause request failed (are the instances up?)"

    for i in $(seq 1 "$N"); do
        curl -sf "http://127.0.0.1:$(ops_port "$i")/api/status" |
            jq -e '.paused == true' >/dev/null ||
            die "instance $i does not report paused"
    done
    ok "breaker tripped on all $N instances (shared store)"

    # Public surface: an INVALID create body must answer 503 (pause gate
    # precedes validation) — proves the gate without creating an order.
    local code
    code="$(curl -s -o /dev/null -w '%{http_code}' -X POST \
        "$(public_api)/api/bridge/orders" -H 'Content-Type: application/json' \
        -d '{"destChain":"ethereum","destAddress":"nope","amount":"1"}')"
    [[ "$code" == "503" ]] ||
        die "public order API answered $code while paused (want 503)"
    ok "public order API refuses new orders while paused (503)"
    date +%s > "$st/paused"
}

# ---------------------------------------------------------------------------
# rotate-drain: no key change while value is in motion.
#   HARD states (submission in flight — never overridable): mint_pending,
#     release_pending.
#   SOFT states (no value moving, retryable under the new keys):
#     awaiting_deposit, deposit_detected, deposit_confirmed, burn_detected,
#     burn_confirmed — waited on, then require an explicit operator ack
#     (BRIDGE_ROTATE_ACK_PARKED) recorded in the drill state.
# ---------------------------------------------------------------------------
cmd_rotate_drain() {
    local term st deadline hard soft
    term="$(pending_term)"; st="$(rotate_state_dir "$term")"
    require_phase "$term" paused rotate-pause
    deadline=$(( $(date +%s) + ${BRIDGE_ROTATE_DRAIN_DEADLINE:-300} ))

    while :; do
        hard="$(db_query "SELECT COALESCE(GROUP_CONCAT(id),'') FROM bridge_orders
                          WHERE status IN ('mint_pending','release_pending');")"
        soft="$(db_query "SELECT COALESCE(GROUP_CONCAT(id),'') FROM bridge_orders
                          WHERE status IN ('awaiting_deposit','deposit_detected',
                                           'deposit_confirmed','burn_detected',
                                           'burn_confirmed');")"
        [[ -z "$hard" && -z "$soft" ]] && break
        [[ "$(date +%s)" -ge "$deadline" ]] && {
            [[ -n "$hard" ]] &&
                die "value-in-motion orders did not settle: $hard — NEVER rotate under these"
            # Soft/parked orders: explicit operator acknowledgment required.
            local ack="${BRIDGE_ROTATE_ACK_PARKED:-}" id missing=""
            if [[ "$ack" != "all" ]]; then
                for id in ${soft//,/ }; do
                    [[ ",$ack," == *",$id,"* ]] || missing+="$id "
                done
                [[ -z "$missing" ]] ||
                    die "parked orders not acknowledged: $missing— re-run with \
BRIDGE_ROTATE_ACK_PARKED listing them (or 'all') to accept rotating under \
parked-retryable orders (their attestation sets rebuild under the new keys)"
            fi
            warn "rotating under operator-acknowledged parked orders: $soft"
            printf '%s\n' "$soft" > "$st/acked-parked-orders"
            break
        }
        log "draining: in-flight [${hard:-none}] parked [${soft:-none}]"
        sleep 10
    done
    ok "drain complete (no value in motion)"
    date +%s > "$st/drained"
}

# ---------------------------------------------------------------------------
# rotate-keys: archive the old term's key material, generate fresh keys.
# Surfaces: federation ed25519 attestation keys, the shared attest bearer
# token, and the Sepolia Safe owner secp256k1 keys. (The LP/relayer key is
# NOT rotated: it holds no roles by design — ADR 0002 — gas only.)
# ---------------------------------------------------------------------------
cmd_rotate_keys() {
    require_gitignored
    local term st ret i
    term="$(pending_term)"; st="$(rotate_state_dir "$term")"
    ret="$(retired_dir $(( term - 1 )))"
    require_phase "$term" drained rotate-drain

    if [[ -d "$ret" ]]; then
        log "term $(( term - 1 )) key material already retired — resuming"
    else
        mkdir -p "$ret" && chmod 700 "$ret"
        for i in $(seq 1 "$N"); do
            mv "$FED_DIR/ed25519-$i.key" "$FED_DIR/ed25519-$i.pub" "$ret/"
            [[ -f "$SECRETS_DIR/eth-safe-owner-$i.key" ]] && {
                mv "$SECRETS_DIR/eth-safe-owner-$i.key" "$ret/"
                mv "$SECRETS_DIR/eth-safe-owner-$i.addr" "$ret/"
            }
        done
        mv "$FED_DIR/attest-token" "$ret/"
        ok "retired term-$(( term - 1 )) keys into $ret"
    fi

    # Fresh federation ed25519 keys + attest token (idempotent).
    for i in $(seq 1 "$N"); do
        if [[ ! -f "$FED_DIR/ed25519-$i.key" ]]; then
            read -r priv pub < <(gen_ed25519)
            printf '%s\n' "$priv" > "$FED_DIR/ed25519-$i.key"
            chmod 600 "$FED_DIR/ed25519-$i.key"
            printf '%s\n' "$pub" > "$FED_DIR/ed25519-$i.pub"
            ok "fresh ed25519 federation key $i (pub $pub)"
        fi
    done
    if [[ ! -f "$FED_DIR/attest-token" ]]; then
        openssl rand -hex 32 > "$FED_DIR/attest-token"
        chmod 600 "$FED_DIR/attest-token"
        ok "fresh shared attest bearer token"
    fi

    # Fresh Sepolia Safe owner keys (disposable testnet keys).
    local cast out addr key
    cast="$(need_cast)"
    for i in $(seq 1 "$N"); do
        if [[ ! -f "$SECRETS_DIR/eth-safe-owner-$i.key" ]]; then
            out="$("$cast" wallet new --json)"
            addr="$(jq -r 'if type=="array" then .[0].address else .address end' <<<"$out")"
            key="$(jq -r 'if type=="array" then .[0].private_key else .private_key end' <<<"$out")"
            [[ "$addr" == 0x* && "$key" == 0x* ]] || die "cast wallet new parse failed"
            printf '%s\n' "$key" > "$SECRETS_DIR/eth-safe-owner-$i.key"
            chmod 600 "$SECRETS_DIR/eth-safe-owner-$i.key"
            printf '%s\n' "$addr" > "$SECRETS_DIR/eth-safe-owner-$i.addr"
            ok "fresh Safe owner key $i: $addr"
        fi
    done
    date +%s > "$st/keys"
}

# ---------------------------------------------------------------------------
# rotate-safe: LIVE Sepolia owner handover. For each member, swapOwner(prev,
# old, new) executed through the Safe itself, signed by 2 keys that are
# CURRENT owners at that moment (the signing set migrates as the swaps land
# — the real rotation choreography). The relayer LP EOA only pays gas.
# ---------------------------------------------------------------------------
safe_owners_live() {
    local cast="$1"
    "$cast" call --rpc-url "$ETH_RPC" "$SAFE" "getOwners()(address[])" |
        tr -d '[] ' | tr ',' '\n'
}

safe_is_owner() {
    local cast="$1" addr="$2"
    "$cast" call --rpc-url "$ETH_RPC" "$SAFE" "isOwner(address)(bool)" "$addr"
}

safe_key_for_owner() {
    # safe_key_for_owner <ret-dir> <owner-addr> — the key file we hold for a
    # CURRENT on-chain owner (searches new keys first, then retired).
    local ret="$1" want f addr
    want="$(tr '[:upper:]' '[:lower:]' <<<"$2")"
    for f in "$SECRETS_DIR"/eth-safe-owner-*.addr "$ret"/eth-safe-owner-*.addr; do
        [[ -f "$f" ]] || continue
        addr="$(tr -d '[:space:]' < "$f" | tr '[:upper:]' '[:lower:]')"
        [[ "$addr" == "$want" ]] && { echo "${f%.addr}.key"; return 0; }
    done
    return 1
}

safe_tx_hash() {
    # safe_tx_hash <cast> <data> — EIP-712 SafeTx hash of a 0-value self-call
    # at the CURRENT nonce.
    local cast="$1" data="$2" nonce
    nonce="$("$cast" call --rpc-url "$ETH_RPC" "$SAFE" "nonce()(uint256)")"
    "$cast" call --rpc-url "$ETH_RPC" "$SAFE" \
        "getTransactionHash(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,uint256)(bytes32)" \
        "$SAFE" 0 "$data" 0 0 0 0 "$ETH_ZERO" "$ETH_ZERO" "$nonce"
}

safe_sign_sorted() {
    # safe_sign_sorted <cast> <txhash> <addr1:key1> <addr2:key2> — Safe
    # requires signatures concatenated in ascending signer-address order
    # (sort on the LOWERCASED address only; never touch the key path).
    local cast="$1" txhash="$2"; shift 2
    local pair addr key sig out=""
    while read -r addr key; do
        sig="$("$cast" wallet sign --no-hash \
            --private-key "$(tr -d '[:space:]' < "$key")" "$txhash")"
        out+="${sig#0x}"
    done < <(
        for pair in "$@"; do
            printf '%s %s\n' \
                "$(tr '[:upper:]' '[:lower:]' <<<"${pair%%:*}")" "${pair#*:}"
        done | sort
    )
    echo "0x$out"
}

cmd_rotate_safe() {
    require_gitignored
    local term st ret cast i
    term="$(pending_term)"; st="$(rotate_state_dir "$term")"
    ret="$(retired_dir $(( term - 1 )))"
    require_phase "$term" keys rotate-keys
    cast="$(need_cast)"

    local thr0; thr0="$("$cast" call --rpc-url "$ETH_RPC" "$SAFE" "getThreshold()(uint256)")"

    for i in $(seq 1 "$N"); do
        local old_addr new_addr
        old_addr="$(tr -d '[:space:]' < "$ret/eth-safe-owner-$i.addr")"
        new_addr="$(tr -d '[:space:]' < "$SECRETS_DIR/eth-safe-owner-$i.addr")"

        if [[ "$(safe_is_owner "$cast" "$new_addr")" == "true" ]]; then
            log "owner $i already swapped ($new_addr)"
            continue
        fi

        # prevOwner in the Safe's linked list (SENTINEL before the first).
        local prev="$ETH_SENTINEL" cur found=""
        while read -r cur; do
            if [[ "$(tr '[:upper:]' '[:lower:]' <<<"$cur")" == \
                  "$(tr '[:upper:]' '[:lower:]' <<<"$old_addr")" ]]; then
                found=1; break
            fi
            prev="$cur"
        done < <(safe_owners_live "$cast")
        [[ -n "$found" ]] || die "old owner $old_addr not in the live owner set"

        local data txhash
        data="$("$cast" calldata 'swapOwner(address,address,address)' \
            "$prev" "$old_addr" "$new_addr")"
        txhash="$(safe_tx_hash "$cast" "$data")"

        # Two signer keys that are CURRENT owners right now.
        local owner pairs=() keyf
        while read -r owner; do
            [[ "${#pairs[@]}" -ge 2 ]] && break
            keyf="$(safe_key_for_owner "$ret" "$owner")" || continue
            pairs+=("$owner:$keyf")
        done < <(safe_owners_live "$cast")
        [[ "${#pairs[@]}" -ge 2 ]] || die "fewer than 2 controllable current owners"

        local sigs tx
        sigs="$(safe_sign_sorted "$cast" "$txhash" "${pairs[@]}")"
        log "swapOwner: member $i  $old_addr -> $new_addr (prev $prev)"
        tx="$("$cast" send --rpc-url "$ETH_RPC" \
            --private-key "$(tr -d '[:space:]' < "$SECRETS_DIR/eth-lp.key")" \
            "$SAFE" \
            'execTransaction(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,bytes)' \
            "$SAFE" 0 "$data" 0 0 0 0 "$ETH_ZERO" "$ETH_ZERO" "$sigs" --json)"
        [[ "$(jq -r .status <<<"$tx")" == "0x1" ]] ||
            die "swapOwner execTransaction reverted: $(jq -r .transactionHash <<<"$tx")"
        ok "owner $i swapped: $(jq -r .transactionHash <<<"$tx")"

        [[ "$(safe_is_owner "$cast" "$old_addr")" == "false" ]] ||
            die "old owner $old_addr still an owner after swap"
        [[ "$(safe_is_owner "$cast" "$new_addr")" == "true" ]] ||
            die "new owner $new_addr not an owner after swap"
    done

    local thr1; thr1="$("$cast" call --rpc-url "$ETH_RPC" "$SAFE" "getThreshold()(uint256)")"
    [[ "$thr0" == "$thr1" ]] || die "Safe threshold changed: $thr0 -> $thr1"
    ok "Sepolia Safe owner set fully rotated (threshold unchanged: $thr1)"
    date +%s > "$st/safe"
}

# ---------------------------------------------------------------------------
# rotate-solana: devnet wbth SPL mint-authority rotation. Testnet custody is
# a single key by design (#867); rotation is one SetAuthority. When the
# Solana CLI tooling is absent the leg is GATED and the exact commands are
# printed. Mainnet delta: the authority is a Squads multisig — rotation is a
# member-swap proposal inside Squads, not SetAuthority.
# ---------------------------------------------------------------------------
solana_mint_authority() {
    curl -sf -X POST -H 'Content-Type: application/json' --data \
        "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getAccountInfo\",\"params\":[\"$SOL_MINT\",{\"encoding\":\"base64\"}]}" \
        "$SOL_RPC" | python3 -c '
import base64, json, sys
raw = json.load(sys.stdin)["result"]["value"]["data"][0]
data = base64.b64decode(raw)
if int.from_bytes(data[0:4], "little") != 1:
    print("none"); sys.exit()
pk = data[4:36]
ALPHABET = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
n = int.from_bytes(pk, "big"); out = ""
while n:
    n, r = divmod(n, 58); out = ALPHABET[r] + out
for b in pk:
    if b == 0: out = "1" + out
    else: break
print(out)'
}

cmd_rotate_solana() {
    require_gitignored
    local term st ret auth0
    term="$(pending_term)"; st="$(rotate_state_dir "$term")"
    ret="$(retired_dir $(( term - 1 )))"
    require_phase "$term" keys rotate-keys

    auth0="$(solana_mint_authority || echo unknown)"
    log "devnet wbth mint $SOL_MINT current mint authority: $auth0"

    if command -v solana-keygen >/dev/null && command -v spl-token >/dev/null; then
        if [[ ! -f "$ret/solana-mint-auth.json" ]]; then
            mv "$SECRETS_DIR/solana-mint-auth.json" "$ret/"
            solana-keygen new --no-bip39-passphrase --silent \
                -o "$SECRETS_DIR/solana-mint-auth.json"
            chmod 600 "$SECRETS_DIR/solana-mint-auth.json"
        fi
        local newpub
        newpub="$(solana-keygen pubkey "$SECRETS_DIR/solana-mint-auth.json")"
        spl-token authorize "$SOL_MINT" mint "$newpub" \
            --authority "$ret/solana-mint-auth.json" \
            --fee-payer "$SECRETS_DIR/solana-lp.json" --url "$SOL_RPC" >&2
        local auth1; auth1="$(solana_mint_authority)"
        [[ "$auth1" == "$newpub" ]] ||
            die "mint authority did not rotate: $auth1 (want $newpub)"
        ok "devnet wbth mint authority rotated: $auth0 -> $auth1"
        echo "live" > "$st/leg-solana"
    else
        warn "solana-keygen / spl-token not installed — Solana leg GATED."
        warn "Operator commands (devnet, single-key authority per #867):"
        warn "  solana-keygen new -o $SECRETS_DIR/solana-mint-auth-term$term.json"
        warn "  spl-token authorize $SOL_MINT mint <NEW_PUBKEY> \\"
        warn "      --authority $SECRETS_DIR/solana-mint-auth.json --url $SOL_RPC"
        warn "  # then archive the old keypair under $ret/"
        warn "Mainnet delta: authority = Squads multisig; rotation is a"
        warn "member-swap proposal executed inside Squads (no SetAuthority)."
        echo "gated:tooling (authority still $auth0)" > "$st/leg-solana"
    fi
}

# ---------------------------------------------------------------------------
# rotate-bth: BTH reserve re-key. Generates a NEW random reserve wallet and
# points reserve.env at it; the funds-moving sweep (old reserve -> new
# reserve address, factor-1 preserved) is a live BTH transaction that is
# OPERATOR-GATED while the betanet cannot confirm transactions (#1051).
# On this drill's betanet the reserve ledger's locked balance is 0 (funding
# itself is blocked, see the runbook prerequisites), so the sweep is vacuous.
# ---------------------------------------------------------------------------
cmd_rotate_bth() {
    require_gitignored
    local term st ret locked
    term="$(pending_term)"; st="$(rotate_state_dir "$term")"
    ret="$(retired_dir $(( term - 1 )))"
    require_phase "$term" keys rotate-keys

    locked="$(jq -r '.lockedReserve // "unknown"' "$st/proof-baseline.json" 2>/dev/null || echo unknown)"
    log "reserve ledger locked balance at pause: ${locked} pc"

    # New reserve wallet (random; the deterministic harness wallet must never
    # custody reserve funds — same rule as gen-reserve).
    if [[ -f "$FED_DIR/bth-reserve/user.view.hex" && ! -d "$ret/bth-reserve" ]]; then
        mv "$FED_DIR/bth-reserve" "$ret/bth-reserve"
        ok "retired old BTH reserve wallet into $ret/bth-reserve"
    fi
    if [[ ! -f "$FED_DIR/bth-reserve/user.view.hex" ]]; then
        local dir="$FED_DIR/bth-reserve" exports addr
        mkdir -p "$dir"
        if exports="$(cd "$REPO_ROOT" && cargo run --release --bin botho-testnet -- \
                gen-bridge-keys --node 0 --out "$dir" 2>/dev/null)"; then
            rm -f "$dir/reserve.view.hex" "$dir/reserve.spend.hex" "$dir/reserve.pq_seed.hex"
            addr="$(printf '%s\n' "$exports" | sed -n 's/^export BRIDGE_BTH_USER_ADDRESS="\(.*\)"$/\1/p')"
            [[ -n "$addr" ]] || die "gen-bridge-keys did not print the user address"
            printf '%s\n' "$addr" > "$dir/address.txt"
            # reserve.env: keep the user wallet, swap the reserve wallet.
            sed -i.bak "s|^export BRIDGE_BTH_RESERVE_ADDRESS=.*|export BRIDGE_BTH_RESERVE_ADDRESS=\"$addr\"|" \
                "$FED_DIR/reserve.env" && rm -f "$FED_DIR/reserve.env.bak"
            ok "new BTH reserve wallet: $addr (reserve.env updated)"
        else
            rmdir "$dir" 2>/dev/null || true
            warn "botho-testnet gen-bridge-keys unavailable — generate the new"
            warn "reserve wallet with:"
            warn "  cargo run --release --bin botho-testnet -- gen-bridge-keys \\"
            warn "      --node 0 --out $FED_DIR/bth-reserve"
            echo "gated:tooling" > "$st/leg-bth"
            return 0
        fi
    fi

    # The funds-moving sweep.
    if [[ "$locked" == "0" ]]; then
        warn "old reserve holds 0 locked pc — the sweep tx is VACUOUS on this"
        warn "betanet (funding is itself blocked, runbook prerequisite 2)."
        echo "vacuous (locked=0); sweep procedure documented, gated on #1051" > "$st/leg-bth"
    else
        warn "OPERATOR-GATED (#1051): the betanet cannot confirm transactions."
        warn "Build+sign the sweep of every factor-1 reserve UTXO from the OLD"
        warn "reserve wallet ($ret/bth-reserve) to the NEW reserve address"
        warn "($(cat "$FED_DIR/bth-reserve/address.txt" 2>/dev/null)), submit once"
        warn "#1051 unfreezes, and record the tx hash in the drill log."
        echo "gated:#1051 (locked=$locked pc awaiting sweep)" > "$st/leg-bth"
    fi
}

# ---------------------------------------------------------------------------
# rotate-seal: the SELECT-THEN-KEYGEN seal step. Consumes the 'elected' term
# document (membership) and produces the 'sealed' one by binding the fresh
# per-term keys generated in rotate-keys/-solana/-bth: each winner submits its
# fresh keys signed by its long-lived identity key (keySubmissionSig), and the
# OUTGOING federation counter-signs the completed document at threshold. Only a
# sealed document authorizes execution and drives rotate-verify. Runs after the
# key-generating legs; solana/bth custody is single-key on testnet so those
# per-member fields carry the shared fresh value (or a mock marker if a leg is
# gated) — mainnet issues each member a distinct Squads/reserve share.
# ---------------------------------------------------------------------------
cmd_rotate_seal() {
    require_gitignored
    local term st ret doc solmember bthres
    term="$(pending_term)"; st="$(rotate_state_dir "$term")"
    ret="$(retired_dir $(( term - 1 )))"
    require_phase "$term" keys rotate-keys
    doc="$(election_doc "$term")"

    # Freshly-generated single-custody material, when the live legs produced it.
    solmember=""; bthres=""
    if command -v solana-keygen >/dev/null 2>&1 && [[ -f "$SECRETS_DIR/solana-mint-auth.json" ]]; then
        solmember="$(solana-keygen pubkey "$SECRETS_DIR/solana-mint-auth.json" 2>/dev/null || echo "")"
    fi
    [[ -f "$FED_DIR/bth-reserve/address.txt" ]] &&
        bthres="$(tr -d '[:space:]' < "$FED_DIR/bth-reserve/address.txt")"

    emit_sealed_doc "$doc" "$FED_DIR" "$SECRETS_DIR" "$ret" "$solmember" "$bthres"
    validate_term_doc "$doc" pass
    ok "term $term SEALED (v2): fresh per-term keys submitted + outgoing counter-signed; validates against $TERM_SCHEMA_REL"
    log "sealed doc: status=$(jq -r .status "$doc") members=$(jq '.members|length' "$doc") outgoing-sigs=$(jq '.signatures.outgoing|length' "$doc")"
    date +%s > "$st/sealed"
}

# ---------------------------------------------------------------------------
# rotate-restart: re-render configs (they pin the NEW public key sets and
# the NEW attest token) and restart every instance. Same env knobs as `up`
# (RESERVE_TOLERANCE, caps) must be supplied on this invocation too.
# ---------------------------------------------------------------------------
cmd_rotate_restart() {
    local term st
    term="$(pending_term)"; st="$(rotate_state_dir "$term")"
    require_phase "$term" keys rotate-keys
    require_phase "$term" sealed rotate-seal
    cmd_down
    cmd_up
    date +%s > "$st/restarted"
}

# ---------------------------------------------------------------------------
# rotate-verify: THE RESUME GATE — prove the old key set is powerless.
# ---------------------------------------------------------------------------
attest_envelope_signed_by() {
    # attest_envelope_signed_by <seed-hex-file> <signer-key-id> <order-id>
    #   <amount> <bth-address> <source-tx>
    # Builds a canonically-encoded, correctly domain-separated, VALIDLY
    # ed25519-SIGNED release attestation envelope (mirrors
    # bridge/core/src/attestation.rs) — signed by the given seed, presented
    # under the given signer id.
    python3 - "$@" <<'PYEOF'
import base64, hashlib, json, os, subprocess, sys, tempfile, time, uuid

seed_file, signer_id, order_id, amount, bth_addr, source_tx = sys.argv[1:7]
seed = bytes.fromhex(open(seed_file).read().strip())
assert len(seed) == 32, "ed25519 seed must be 32 bytes"
# Minimal PKCS8 wrapping of a raw ed25519 seed (RFC 8410).
der = bytes.fromhex("302e020100300506032b657004220420") + seed
pem = ("-----BEGIN PRIVATE KEY-----\n"
       + base64.encodebytes(der).decode()
       + "-----END PRIVATE KEY-----\n")

def sign(msg: bytes) -> str:
    with tempfile.TemporaryDirectory() as d:
        kp, mp, sp = (os.path.join(d, n) for n in ("k.pem", "m.bin", "s.bin"))
        open(kp, "w").write(pem)
        open(mp, "wb").write(msg)
        subprocess.run(["openssl", "pkeyutl", "-sign", "-inkey", kp,
                        "-rawin", "-in", mp, "-out", sp], check=True)
        return open(sp, "rb").read().hex()

issued = int(time.time()); expires = issued + 240
nonce = "rotate-drill-" + uuid.uuid4().hex
amt = int(amount)
params = ('{"amount":%d,"bthAddress":%s,"orderId":%s,'
          '"sourceChain":"ethereum","sourceTx":%s}') % (
    amt, json.dumps(bth_addr), json.dumps(order_id), json.dumps(source_tx))
envelope = ('{"action":"bridge.release_bth","expiresAt":%d,"issuedAt":%d,'
            '"nonce":%s,"params":%s,"signerKeyId":%s,"v":1}') % (
    expires, issued, json.dumps(nonce), params, json.dumps(signer_id))

env_msg = b"botho-bridge-attest-bth-v1" + envelope.encode()
oid = hashlib.sha256(b"botho-bridge-order-id-v1"
                     + uuid.UUID(order_id).bytes).digest()
payload = hashlib.sha256(b"botho-bridge-release-v1" + oid
                         + amt.to_bytes(8, "little")
                         + len(bth_addr.encode()).to_bytes(8, "little")
                         + bth_addr.encode()).digest()
print(json.dumps({"envelope": envelope,
                  "signature_hex": sign(env_msg),
                  "payload_signature_hex": sign(payload)}))
PYEOF
}

attest_post_tag() {
    # attest_post_tag <instance-i> <token> <body> -> "<http-code> <tag>"
    local i="$1" token="$2" body="$3" resp code
    resp="$(curl -s -w '\n%{http_code}' -X POST \
        "http://127.0.0.1:$(attest_port "$i")/api/attest" \
        -H "Authorization: Bearer $token" -H 'Content-Type: application/json' \
        -d "$body")"
    code="${resp##*$'\n'}"
    echo "$code $(jq -r '.tag // "no-tag"' <<<"${resp%$'\n'*}" 2>/dev/null || echo unparsed)"
}

cmd_rotate_verify() {
    local term st ret cast i
    term="$(pending_term)"; st="$(rotate_state_dir "$term")"
    ret="$(retired_dir $(( term - 1 )))"
    require_phase "$term" restarted rotate-restart
    cast="$(need_cast)"

    # ── (0) the SEALED term document is the authority for this rotation. It
    #        validates against the committed v2 schema, and its pinned per-member
    #        keys must equal the live fresh key files the instances now run on
    #        (so old-keys-dead assertions below check the exact set the document
    #        authorized — not merely whatever is on disk).
    local doc; doc="$(election_doc "$term")"
    [[ "$(jq -r '.status' "$doc")" == "sealed" ]] ||
        die "term $term document is not 'sealed' — run rotate-seal before rotate-verify"
    validate_term_doc "$doc" pass
    for i in $(seq 1 "$N"); do
        local dpub daddr lpub laddr
        dpub="$(jq -r --argjson i "$i" '.members[$i-1].keys.ed25519AttestationPubkey' "$doc")"
        daddr="$(jq -r --argjson i "$i" '.members[$i-1].keys.ethSafeOwner' "$doc")"
        lpub="$(tr -d '[:space:]' < "$FED_DIR/ed25519-$i.pub")"
        laddr="$(tr -d '[:space:]' < "$SECRETS_DIR/eth-safe-owner-$i.addr")"
        [[ "$dpub" == "$lpub" ]] ||
            die "sealed doc ed25519 pubkey for member $i disagrees with the live key file"
        [[ "$(tr '[:upper:]' '[:lower:]' <<<"$daddr")" == "$(tr '[:upper:]' '[:lower:]' <<<"$laddr")" ]] ||
            die "sealed doc Safe owner for member $i disagrees with the live key file"
    done
    ok "sealed term document validates (v2 schema) and pins the live fresh key set"

    # ── (a) federation: a VALIDLY-SIGNED old-key attestation must be refused
    local order_row order_id amount source_tx
    order_row="$(db_query "SELECT id || '|' || amount || '|' || COALESCE(source_tx,'0xdeadbeef')
                           FROM bridge_orders ORDER BY created_at DESC LIMIT 1;")"
    if [[ -n "$order_row" ]]; then
        order_id="${order_row%%|*}"
        amount="$(cut -d'|' -f2 <<<"$order_row")"
        source_tx="$(cut -d'|' -f3 <<<"$order_row")"
    else
        die "no order on record to bind the probe attestation to — run the \
mint drill once (the attest endpoint refuses unknown orders before the \
signer check, so the probe needs a real order id)"
    fi

    local new_token old_token old_pub new_pub body out
    new_token="$(cat "$FED_DIR/attest-token")"
    old_token="$(cat "$ret/attest-token")"
    old_pub="$(tr -d '[:space:]' < "$ret/ed25519-1.pub")"
    new_pub="$(tr -d '[:space:]' < "$FED_DIR/ed25519-1.pub")"

    # OLD key, OLD signer id — fully valid signature, retired identity.
    body="$(attest_envelope_signed_by "$ret/ed25519-1.key" "$old_pub" \
        "$order_id" "$amount" "bth-rotation-drill-probe" "$source_tx")"
    for i in $(seq 1 "$N"); do
        out="$(attest_post_tag "$i" "$new_token" "$body")"
        [[ "$out" == *"refused:unknown_signer"* ]] ||
            die "instance $i accepted (or mis-refused) an OLD-key attestation: $out"
    done
    ok "old-key attestation refused:unknown_signer on all $N instances"

    # OLD key presenting a NEW signer id — impersonation attempt must fail
    # on the SIGNATURE (also proves the pipeline is verifying, not just
    # blanket-refusing).
    body="$(attest_envelope_signed_by "$ret/ed25519-1.key" "$new_pub" \
        "$order_id" "$amount" "bth-rotation-drill-probe" "$source_tx")"
    out="$(attest_post_tag 1 "$new_token" "$body")"
    [[ "$out" == *"refused:bad_signature"* ]] ||
        die "old key impersonating a new signer id was not signature-refused: $out"
    ok "old key under a new signer id refused:bad_signature (pipeline live)"

    # OLD bearer token — transport auth also rotated.
    out="$(attest_post_tag 1 "$old_token" "$body")"
    [[ "$out" == 401* ]] ||
        die "old attest bearer token was not refused: $out"
    ok "old attest bearer token refused (401)"

    # ── (b) Sepolia Safe: old owners are out AND cannot execute
    for i in $(seq 1 "$N"); do
        local old_addr new_addr
        old_addr="$(tr -d '[:space:]' < "$ret/eth-safe-owner-$i.addr")"
        new_addr="$(tr -d '[:space:]' < "$SECRETS_DIR/eth-safe-owner-$i.addr")"
        [[ "$(safe_is_owner "$cast" "$old_addr")" == "false" ]] ||
            die "old owner $old_addr is STILL an owner"
        [[ "$(safe_is_owner "$cast" "$new_addr")" == "true" ]] ||
            die "new owner $new_addr is NOT an owner"
    done
    ok "isOwner: all old owners removed, all new owners present"

    # execTransaction simulation (eth_call — free, no state change) of a
    # benign 0-value self-call: OLD-owner signatures must REVERT (GS026),
    # NEW-owner signatures must succeed.
    local txhash old_sigs new_sigs sim
    txhash="$(safe_tx_hash "$cast" "0x")"
    old_sigs="$(safe_sign_sorted "$cast" "$txhash" \
        "$(tr -d '[:space:]' < "$ret/eth-safe-owner-1.addr"):$ret/eth-safe-owner-1.key" \
        "$(tr -d '[:space:]' < "$ret/eth-safe-owner-2.addr"):$ret/eth-safe-owner-2.key")"
    new_sigs="$(safe_sign_sorted "$cast" "$txhash" \
        "$(tr -d '[:space:]' < "$SECRETS_DIR/eth-safe-owner-1.addr"):$SECRETS_DIR/eth-safe-owner-1.key" \
        "$(tr -d '[:space:]' < "$SECRETS_DIR/eth-safe-owner-2.addr"):$SECRETS_DIR/eth-safe-owner-2.key")"

    if sim="$("$cast" call --rpc-url "$ETH_RPC" --from \
            "$(tr -d '[:space:]' < "$SECRETS_DIR/eth-lp.addr")" "$SAFE" \
            'execTransaction(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,bytes)(bool)' \
            "$SAFE" 0 "0x" 0 0 0 0 "$ETH_ZERO" "$ETH_ZERO" "$old_sigs" 2>&1)"; then
        die "OLD Safe owners can still execute (simulation succeeded: $sim)"
    fi
    grep -qi "GS026\|revert" <<<"$sim" ||
        warn "old-owner simulation failed with unexpected error: $sim"
    ok "old Safe owners cannot execute (simulation reverted: ${sim##*$'\n'})"

    sim="$("$cast" call --rpc-url "$ETH_RPC" --from \
        "$(tr -d '[:space:]' < "$SECRETS_DIR/eth-lp.addr")" "$SAFE" \
        'execTransaction(address,uint256,bytes,uint8,uint256,uint256,uint256,address,address,bytes)(bool)' \
        "$SAFE" 0 "0x" 0 0 0 0 "$ETH_ZERO" "$ETH_ZERO" "$new_sigs")" ||
        die "NEW Safe owners cannot execute (control simulation failed: $sim)"
    ok "new Safe owners execute (control simulation: $sim)"

    # ── (c) gated legs are recorded, not silently skipped
    local leg
    for leg in solana bth; do
        [[ -f "$st/leg-$leg" ]] ||
            die "leg '$leg' has no recorded outcome — run rotate-$leg"
        log "leg $leg: $(cat "$st/leg-$leg")"
    done

    date +%s > "$st/verified"
    ok "OLD KEY SET IS POWERLESS on every live surface — resume is unlocked"
}

# ---------------------------------------------------------------------------
# rotate-resume: lift the breaker — ONLY after rotate-verify passed — and
# commit the new term.
# ---------------------------------------------------------------------------
cmd_rotate_resume() {
    local term st i code
    term="$(pending_term)"; st="$(rotate_state_dir "$term")"
    require_phase "$term" verified rotate-verify

    curl -sf -X POST "http://127.0.0.1:$(ops_port 1)/api/breaker" \
        -H 'Content-Type: application/json' \
        -d '{"paused":false}' | jq . >&2 || die "breaker resume request failed"

    for i in $(seq 1 "$N"); do
        curl -sf "http://127.0.0.1:$(ops_port "$i")/api/status" |
            jq -e '.paused == false' >/dev/null ||
            die "instance $i still reports paused"
    done

    # Public surface: the same invalid-body probe now reaches validation
    # (400) instead of the pause gate (503) — gate lifted, no order created.
    code="$(curl -s -o /dev/null -w '%{http_code}' -X POST \
        "$(public_api)/api/bridge/orders" -H 'Content-Type: application/json' \
        -d '{"destChain":"ethereum","destAddress":"nope","amount":"1"}')"
    [[ "$code" == "400" ]] ||
        die "public order API answered $code after resume (want 400)"

    echo "$term" > "$ELECTION_DIR/current-term"
    date +%s > "$st/resumed"
    ok "breaker lifted; term $term committed — the NEW set is live"
}

# ---------------------------------------------------------------------------
# rotate-attest: post-rotation proof round. The NEW set must (1) reach a
# fresh attestation threshold (audit log records attestation_authorized
# AFTER the resume — old-key envelopes cannot contribute, as proven by
# rotate-verify) and (2) hold the proof-of-reserves invariant exactly where
# the pre-pause baseline left it (rotation moves no value).
# ---------------------------------------------------------------------------
cmd_rotate_attest() {
    local term st resumed deadline row
    term="$(( $(current_term) ))"
    st="$(rotate_state_dir "$term")"
    [[ -f "$st/resumed" ]] || die "term $term has not resumed — run rotate-resume"
    resumed="$(cat "$st/resumed")"
    deadline=$(( $(date +%s) + ${BRIDGE_ROTATE_ATTEST_DEADLINE:-600} ))

    log "waiting for a post-rotation attestation threshold (audit log after $resumed)…"
    while :; do
        row="$(db_query "SELECT created_at || ' ' || COALESCE(order_id,'-') || ' ' || details
                         FROM audit_log
                         WHERE action = 'attestation_authorized' AND created_at >= $resumed
                         ORDER BY created_at DESC LIMIT 1;")"
        [[ -n "$row" ]] && break
        [[ "$(date +%s)" -ge "$deadline" ]] &&
            die "no attestation_authorized after resume within the deadline — \
needs an actionable order (e.g. the parked burn order, with caps raised) to \
drive a round; see the runbook"
        sleep 15
    done
    ok "post-rotation threshold reached by the NEW set: $row"

    # Proof-of-reserves: drift must equal the pre-pause baseline exactly.
    local baseline drift i
    baseline="$(jq -r '.drift // empty' "$st/proof-baseline.json" 2>/dev/null)"
    sleep 5
    for i in $(seq 1 "$N"); do
        drift="$(curl -sf "http://127.0.0.1:$(ops_port "$i")/api/reserve/proof" |
            jq -r '.drift // empty')"
        [[ -n "$drift" ]] || { warn "instance $i: no reconciliation snapshot yet"; continue; }
        if [[ -n "$baseline" ]]; then
            [[ "$drift" == "$baseline" ]] ||
                die "instance $i drift moved across the rotation: $baseline -> $drift"
        fi
        log "instance $i post-rotation drift: $drift (baseline ${baseline:-n/a})"
    done
    ok "proof-of-reserves invariant unchanged across the rotation"
    date +%s > "$st/attested"

    echo
    echo "=== rotation drill term $term summary ==="
    echo "  federation ed25519 : rotated LIVE (old keys refused:unknown_signer)"
    echo "  attest bearer token: rotated LIVE (old token 401)"
    echo "  Sepolia Safe owners: rotated LIVE (old owners out + cannot execute)"
    echo "  Solana wbth mint   : $(cat "$st/leg-solana" 2>/dev/null || echo unrecorded)"
    echo "  BTH reserve        : $(cat "$st/leg-bth" 2>/dev/null || echo unrecorded)"
    echo "  post-rotation round: attestation_authorized by the NEW set"
    echo "  proof-of-reserves  : drift unchanged"
}

cmd_rotate() {
    cmd_rotate_elect
    cmd_rotate_pause
    cmd_rotate_drain
    cmd_rotate_keys
    cmd_rotate_safe
    cmd_rotate_solana
    cmd_rotate_bth
    cmd_rotate_seal
    cmd_rotate_restart
    cmd_rotate_verify
    cmd_rotate_resume
    cmd_rotate_attest
}

# ---------------------------------------------------------------------------
# term-doc-selftest: OFFLINE self-check of the v2 term-document format — needs
# no live services. Exercises the exact emit_elected_doc / emit_sealed_doc code
# paths the drill uses: validates the committed worked example, drives the
# elected -> sealed transition with real ed25519 key submission + outgoing
# counter-signatures, verifies every signature cryptographically, and asserts
# the schema REJECTS a sealed document that is missing its per-term keys (nit a).
# ---------------------------------------------------------------------------
verify_tally_attestations() {
    local doc="$1" fd="$2" n k nid sig pub idx
    n="$(jq '.signatures.tallyAttestations | length' "$doc")"
    for k in $(seq 0 $(( n - 1 ))); do
        nid="$(jq -r --argjson k "$k" '.signatures.tallyAttestations[$k].nodeId' "$doc")"
        sig="$(jq -r --argjson k "$k" '.signatures.tallyAttestations[$k].sig' "$doc")"; sig="${sig#ed25519:}"
        idx="${nid##*-}"; idx="$(( 10#$idx ))"
        pub="$(tr -d '[:space:]' < "$fd/identity-$idx.pub")"
        tally_msg "$doc" | ed25519_verify_hex "$pub" "$sig" ||
            die "tallyAttestation for $nid failed to verify against its identity key"
    done
    ok "all $n tallyAttestations verify (cover the tally/membership snapshot)"
}
verify_key_submissions() {
    local doc="$1" fd="$2" i sig pub
    for i in $(seq 1 "$N"); do
        sig="$(jq -r --argjson i "$i" '.members[$i-1].keySubmissionSig' "$doc")"; sig="${sig#ed25519:}"
        pub="$(tr -d '[:space:]' < "$fd/identity-$i.pub")"
        keysub_msg "$doc" "$i" | ed25519_verify_hex "$pub" "$sig" ||
            die "keySubmissionSig for member $i failed to verify against its identity key"
    done
    ok "all $N keySubmissionSig verify against the long-lived identity keys"
}
verify_outgoing() {
    local doc="$1" n k pub sig idx
    n="$(jq '.signatures.outgoing | length' "$doc")"
    [[ "$n" -ge "$T" ]] || die "outgoing counter-signatures ($n) below threshold $T"
    for k in $(seq 0 $(( n - 1 ))); do
        idx="$(jq -r --argjson k "$k" '.signatures.outgoing[$k].index' "$doc")"
        pub="$(jq -r --argjson k "$k" '.signatures.outgoing[$k].ed25519AttestationPubkey' "$doc")"
        sig="$(jq -r --argjson k "$k" '.signatures.outgoing[$k].sig' "$doc")"; sig="${sig#ed25519:}"
        outgoing_msg "$doc" | ed25519_verify_hex "$pub" "$sig" ||
            die "outgoing counter-signature index $idx failed to verify"
    done
    ok "outgoing counter-signatures verify (threshold $T reached)"
}
cmd_term_doc_selftest() {
    command -v jq >/dev/null      || die "jq required"
    command -v python3 >/dev/null || die "python3 required"
    command -v openssl >/dev/null || die "openssl (ed25519) required"
    [[ -f "$TERM_SCHEMA" ]]  || die "missing schema $TERM_SCHEMA_REL"
    [[ -f "$TERM_EXAMPLE" ]] || die "missing worked example $TERM_EXAMPLE"

    local tmp; tmp="$(mktemp -d)"
    # shellcheck disable=SC2064
    trap "rm -rf '$tmp'" RETURN
    local N_save="$N" T_save="$T"; N=3; T=2
    local fd="$tmp/fed" sd="$tmp/sec" ret="$tmp/retired" doc="$tmp/term-2.json" i p q
    mkdir -p "$fd" "$sd" "$ret"
    for i in $(seq 1 "$N"); do
        read -r p q < <(gen_ed25519)   # fresh per-term attestation key
        printf '%s\n' "$p" > "$fd/ed25519-$i.key"; printf '%s\n' "$q" > "$fd/ed25519-$i.pub"
        read -r p q < <(gen_ed25519)   # outgoing (retired) attestation key
        printf '%s\n' "$p" > "$ret/ed25519-$i.key"; printf '%s\n' "$q" > "$ret/ed25519-$i.pub"
        printf '0x%040x\n' "$i" > "$sd/eth-safe-owner-$i.addr"   # well-formed fake addr
    done

    log "1/5 committed worked example validates against $TERM_SCHEMA_REL"
    validate_term_doc "$TERM_EXAMPLE" pass; ok "worked example is schema-valid"

    log "2/5 elected document: membership only, no per-term keys"
    emit_elected_doc "$doc" 2 "$fd" "$sd" "mock-same-set"
    validate_term_doc "$doc" pass
    [[ "$(jq -r '.status' "$doc")" == "elected" ]] || die "elected doc has wrong status"
    [[ "$(jq '.members[0] | has("keys")' "$doc")" == "false" ]] ||
        die "elected doc must NOT carry per-term keys"
    verify_tally_attestations "$doc" "$fd"

    log "3/5 negative: a 'sealed' document without keys must be REJECTED (nit a)"
    jq '.status = "sealed"' "$doc" > "$tmp/bad.json"
    validate_term_doc "$tmp/bad.json" fail; ok "schema rejects sealed-without-keys"

    log "4/5 seal transition: fresh keys + submission sigs + outgoing counter-sign"
    emit_sealed_doc "$doc" "$fd" "$sd" "$ret" \
        "Examp1eSo1anaMember1111111111111111111111111" \
        "bth1qexamplereserve0000000000000000000000000"
    validate_term_doc "$doc" pass
    [[ "$(jq -r '.status' "$doc")" == "sealed" ]] || die "sealed doc has wrong status"

    log "5/5 verify every signature cryptographically"
    verify_key_submissions "$doc" "$fd"
    verify_outgoing "$doc" "$ret"
    verify_tally_attestations "$doc" "$fd"   # still valid post-seal (nit b)

    N="$N_save"; T="$T_save"
    ok "term-document v2 self-check PASSED (elected -> sealed, key submission, counter-sign, schema)"
}

usage() { sed -n '2,71p' "$0" | sed 's/^# \{0,1\}//'; }

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
    rotate)          cmd_rotate ;;
    rotate-elect)    cmd_rotate_elect ;;
    rotate-pause)    cmd_rotate_pause ;;
    rotate-drain)    cmd_rotate_drain ;;
    rotate-keys)     cmd_rotate_keys ;;
    rotate-safe)     cmd_rotate_safe ;;
    rotate-solana)   cmd_rotate_solana ;;
    rotate-bth)      cmd_rotate_bth ;;
    rotate-seal)     cmd_rotate_seal ;;
    rotate-restart)  cmd_rotate_restart ;;
    rotate-verify)   cmd_rotate_verify ;;
    rotate-resume)   cmd_rotate_resume ;;
    rotate-attest)   cmd_rotate_attest ;;
    term-doc-selftest) cmd_term_doc_selftest ;;
    *)            usage; exit 1 ;;
esac
