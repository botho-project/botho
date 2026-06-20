#!/usr/bin/env bash
#
# Botho BaaS Rig Bootstrap (cloud-init / EC2 user-data)
# =====================================================
#
# Self-contained first-boot provisioner that turns a *fresh* EC2 instance
# (Ubuntu 24.04 arm64, e.g. t4g.medium) into a working Botho **testnet mining
# node** with ZERO manual SSH. This is the automation the BaaS provisioner
# (issue #458, epic #441) invokes for each paid signup.
#
# It is the scripted form of the manual seed recipe in
#   infra/seed/{deploy-botho.sh,reset-to-testnet.sh,seed-nginx.conf,botho-seed.service}
#   infra/faucet/faucet-config.toml.template
# transformed into an idempotent, parameterized, non-interactive boot script.
#
# ---------------------------------------------------------------------------
# USAGE
# ---------------------------------------------------------------------------
# As EC2 user-data, paste this file (optionally prefixed with the parameter
# exports below) into the instance launch's "User data" field. cloud-init runs
# it as root on first boot. It is also safe to run by hand:
#
#     sudo RIG_HOSTNAME=rig-demo.testnet.botho.io \
#          BOTHO_BINARY_URL=https://example.com/botho-aarch64 \
#          ./rig-bootstrap.sh
#
# Re-running is safe (idempotent): each step checks current state first.
#
# ---------------------------------------------------------------------------
# PARAMETERS (environment variables / user-data exports)
# ---------------------------------------------------------------------------
#   BOTHO_BINARY_URL   (REQUIRED unless a binary is already installed)
#                      URL to a linux-aarch64 `botho` binary. The provisioner
#                      (#458) must publish the current release artifact (e.g.
#                      upload to S3/R2) and pass its URL here. There is no
#                      public GitHub release asset yet — see BINARY SOURCE note
#                      at the bottom of this file and infra/baas/README.md.
#   BOTHO_BINARY_SHA256 (optional) expected sha256 of the downloaded binary;
#                      verified if set.
#   RIG_HOSTNAME       (optional) public hostname for this rig, e.g.
#                      rig-abc123.testnet.botho.io. The provisioner pre-creates
#                      the DNS A record -> this instance's public IP BEFORE
#                      boot. If unset, TLS/nginx public setup is skipped and the
#                      node still serves RPC on localhost:17101.
#   NETWORK            (optional, default "testnet"). Only "testnet" is
#                      supported by this slice.
#   BOOTSTRAP_PEERS    (optional) comma-separated libp2p multiaddrs to use as
#                      bootstrap_peers. Default: the live seed nodes plus DNS
#                      seed discovery (seeds.testnet.botho.io).
#   MINT_THREADS       (optional, default 1) RandomX minting threads. t4g.medium
#                      has 2 vCPU / ~4GB; 1 thread leaves headroom.
#   CERTBOT_EMAIL      (optional) email for Let's Encrypt registration.
#   TLS_MODE           (optional) "webroot" (default, needs nginx+DNS),
#                      "standalone" (certbot --standalone, stops nginx briefly),
#                      or "skip" (no certbot; HTTP-only nginx for local test).
#   RIG_WALLET_MNEMONIC (optional) bring-your-own 24-word mnemonic. Default:
#                      generate a fresh per-rig mnemonic. (#458 will decide
#                      bring-your-own vs generated; the param exists already.)
#   BIP39_WORDLIST_URL (optional) URL to the BIP39 English wordlist (2048 words,
#                      one per line) used to generate the rig mnemonic. Default:
#                      the canonical bitcoin/bips raw URL. A local copy next to
#                      this script (or at /usr/local/share/botho/) is used first
#                      if present, so user-data stays small.
#
# ---------------------------------------------------------------------------
# OUTPUTS
# ---------------------------------------------------------------------------
#   /var/log/botho-rig-bootstrap.log   full provisioning log
#   /home/ubuntu/.botho/testnet/config.toml   node config (mnemonic, chmod 600)
#   /home/ubuntu/rig-info.txt          machine-readable summary (RPC URL, peer
#                                      id, mnemonic location, status command)
#   systemd unit `botho` running `botho --testnet run --mint`
#   Read back any time with:  sudo /usr/local/bin/rig-status   (installed here)
#
set -euo pipefail

# ---------------------------------------------------------------------------
# Logging: tee everything to a persistent log AND the cloud-init console.
# ---------------------------------------------------------------------------
LOG_FILE="/var/log/botho-rig-bootstrap.log"
exec > >(tee -a "$LOG_FILE") 2>&1

ts()   { date -u +"%Y-%m-%dT%H:%M:%SZ"; }
log()  { echo "[$(ts)] [rig-bootstrap] $*"; }
fail() { echo "[$(ts)] [rig-bootstrap] FATAL: $*" >&2; exit 1; }

log "=== Botho rig bootstrap starting ==="

# ---------------------------------------------------------------------------
# Parameters & defaults
# ---------------------------------------------------------------------------
NETWORK="${NETWORK:-testnet}"
RIG_HOSTNAME="${RIG_HOSTNAME:-}"
BOTHO_BINARY_URL="${BOTHO_BINARY_URL:-}"
BOTHO_BINARY_SHA256="${BOTHO_BINARY_SHA256:-}"
BOOTSTRAP_PEERS="${BOOTSTRAP_PEERS:-}"
MINT_THREADS="${MINT_THREADS:-1}"
CERTBOT_EMAIL="${CERTBOT_EMAIL:-admin@botho.io}"
TLS_MODE="${TLS_MODE:-webroot}"
RIG_WALLET_MNEMONIC="${RIG_WALLET_MNEMONIC:-}"
BIP39_WORDLIST_URL="${BIP39_WORDLIST_URL:-https://raw.githubusercontent.com/bitcoin/bips/master/bip-0039/english.txt}"

# Service account: the Ubuntu arm64 AMI ships an "ubuntu" user; mirror the
# seed/faucet layout that runs the node as that user with data in ~/.botho.
RUN_USER="ubuntu"
RUN_HOME="/home/${RUN_USER}"
DATA_DIR="${RUN_HOME}/.botho/${NETWORK}"
CONFIG_FILE="${DATA_DIR}/config.toml"
BIN_PATH="/usr/local/bin/botho"
RPC_PORT=17101
GOSSIP_PORT=17100

[[ "$NETWORK" == "testnet" ]] || fail "Only NETWORK=testnet is supported in this slice (got '$NETWORK')."
id "$RUN_USER" >/dev/null 2>&1 || fail "Expected user '$RUN_USER' to exist (Ubuntu arm64 AMI)."

log "Params: NETWORK=$NETWORK RIG_HOSTNAME='${RIG_HOSTNAME:-<none>}' TLS_MODE=$TLS_MODE MINT_THREADS=$MINT_THREADS"
log "Binary source: ${BOTHO_BINARY_URL:-<none, will require existing $BIN_PATH>}"

# ===========================================================================
# Step 1: Install dependencies (idempotent)
# ===========================================================================
log "Step 1: installing system dependencies"
export DEBIAN_FRONTEND=noninteractive
NEEDED_PKGS=(nginx certbot python3-certbot-nginx curl ca-certificates jq)
MISSING=()
for p in "${NEEDED_PKGS[@]}"; do
    dpkg -s "$p" >/dev/null 2>&1 || MISSING+=("$p")
done
if [[ ${#MISSING[@]} -gt 0 ]]; then
    log "  installing: ${MISSING[*]}"
    apt-get update -qq
    apt-get install -y -qq "${MISSING[@]}"
else
    log "  all dependencies already present"
fi

# ===========================================================================
# Step 2: Obtain the botho linux-aarch64 binary
# ===========================================================================
# PREFER fetching a published binary over building on the box (t4g release
# builds take ~20+ min and can OOM RandomX-linked crates). The source is the
# BOTHO_BINARY_URL parameter; #458's provisioner publishes the release artifact
# (e.g. to S3/R2) and supplies the URL.
log "Step 2: obtaining botho binary"
install_binary() {
    local url="$1" tmp
    tmp="$(mktemp /tmp/botho.XXXXXX)"
    log "  downloading $url"
    curl -fSL --retry 5 --retry-delay 5 -o "$tmp" "$url" \
        || fail "failed to download botho binary from $url"
    if [[ -n "$BOTHO_BINARY_SHA256" ]]; then
        local got
        got="$(sha256sum "$tmp" | awk '{print $1}')"
        [[ "$got" == "$BOTHO_BINARY_SHA256" ]] \
            || fail "binary sha256 mismatch: got $got expected $BOTHO_BINARY_SHA256"
        log "  sha256 verified: $got"
    fi
    file "$tmp" | grep -q "aarch64" || fail "downloaded binary is not aarch64: $(file "$tmp")"
    install -m 0755 "$tmp" "$BIN_PATH"
    rm -f "$tmp"
}

if [[ -n "$BOTHO_BINARY_URL" ]]; then
    install_binary "$BOTHO_BINARY_URL"
elif [[ -x "$BIN_PATH" ]]; then
    log "  BOTHO_BINARY_URL unset; reusing existing $BIN_PATH (idempotent re-run)"
else
    fail "no BOTHO_BINARY_URL and no existing $BIN_PATH. The provisioner (#458) must publish a linux-aarch64 binary and pass BOTHO_BINARY_URL. See infra/baas/README.md."
fi
# The binary has no `--version` flag; the version is reported via RPC
# (nodeVersion) once the node is up. Record the architecture here; resolve the
# version in Step 6 after the service starts.
if file "$BIN_PATH" | grep -qi "aarch64"; then BIN_ARCH="aarch64"; else BIN_ARCH="unknown-arch"; fi
log "  installed botho ($BIN_ARCH)"

# ===========================================================================
# Step 3: Generate per-rig identity + write config.toml
# ===========================================================================
# Notes on "node_key":
#   The botho binary persists its libp2p peer identity automatically: on first
#   start it creates ~/.botho/<network>/node_key and reloads it on every
#   subsequent boot (log: "Loaded persistent node identity from ... node_key").
#   So the rig's peer id is stable across reboots with no action needed here.
#   The *wallet mnemonic* below is the rig's economic identity (it owns the
#   mined rewards); we generate it once and preserve it across re-runs.
log "Step 3: generating identity + config"
install -d -o "$RUN_USER" -g "$RUN_USER" -m 0755 "$RUN_HOME/.botho"
install -d -o "$RUN_USER" -g "$RUN_USER" -m 0700 "$DATA_DIR"

# Mnemonic: bring-your-own, else reuse existing config's, else generate fresh.
gen_mnemonic() {
    # Generate a BIP39 24-word (256-bit) English mnemonic without needing the
    # interactive `botho init`. Uses the standard wordlist shipped here.
    local words_file="$1"
    python3 - "$words_file" <<'PY'
import hashlib, os, sys
wl = open(sys.argv[1]).read().split()
assert len(wl) == 2048, "bad wordlist"
ent = os.urandom(32)  # 256 bits -> 24 words
h = hashlib.sha256(ent).digest()
bits = ''.join(f'{b:08b}' for b in ent) + ''.join(f'{b:08b}' for b in h)[:8]
idx = [int(bits[i:i+11], 2) for i in range(0, len(bits), 11)]
print(' '.join(wl[i] for i in idx))
PY
}

if [[ -f "$CONFIG_FILE" ]]; then
    log "  config already exists; preserving wallet mnemonic (idempotent)"
    MNEMONIC="$(grep -E '^\s*mnemonic\s*=' "$CONFIG_FILE" | head -1 | sed -E 's/^[^"]*"([^"]*)".*/\1/')"
    [[ -n "$MNEMONIC" ]] || fail "existing config has no mnemonic; refusing to overwrite. Inspect $CONFIG_FILE"
elif [[ -n "$RIG_WALLET_MNEMONIC" ]]; then
    log "  using bring-your-own RIG_WALLET_MNEMONIC"
    MNEMONIC="$RIG_WALLET_MNEMONIC"
else
    log "  generating fresh per-rig wallet mnemonic"
    WORDLIST="$(dirname "$0")/bip39-english.txt"
    [[ -f "$WORDLIST" ]] || WORDLIST="/usr/local/share/botho/bip39-english.txt"
    if [[ ! -f "$WORDLIST" ]]; then
        WORDLIST="/usr/local/share/botho/bip39-english.txt"
        install -d /usr/local/share/botho
        log "  fetching BIP39 wordlist from $BIP39_WORDLIST_URL"
        curl -fSL --retry 5 --retry-delay 3 -o "$WORDLIST" "$BIP39_WORDLIST_URL" \
            || fail "failed to fetch BIP39 wordlist from $BIP39_WORDLIST_URL"
    fi
    [[ "$(wc -l < "$WORDLIST")" -eq 2048 ]] || fail "BIP39 wordlist at $WORDLIST is not 2048 words"
    MNEMONIC="$(gen_mnemonic "$WORDLIST")"
fi

# Bootstrap peers.
#
# IMPORTANT: the node's libp2p transport does NOT support bare `/dns4/.../tcp`
# multiaddrs in the explicit `bootstrap_peers` list (it returns
# MultiaddrNotSupported and never connects). The working path is DNS-seed
# discovery: when `bootstrap_peers` is EMPTY and `dns_seeds.enabled = true`,
# the node queries `seeds.testnet.botho.io` TXT records, resolves them to
# `/ip4/.../tcp/<port>/p2p/<peer_id>` multiaddrs (which the transport DOES
# support), and connects. Verified end-to-end on a fresh t4g.medium (#462).
#
# So:
#   * default  -> leave bootstrap_peers EMPTY and let DNS discovery do the work.
#   * override -> if BOOTSTRAP_PEERS is set, use it verbatim. Provide FULLY
#     RESOLVED multiaddrs WITH a /p2p/<peer_id> suffix, preferably
#     /ip4/<addr>/tcp/<port>/p2p/<peer_id> (bare /dns4 forms will NOT connect).
if [[ -n "$BOOTSTRAP_PEERS" ]]; then
    PEERS_TOML="$(echo "$BOOTSTRAP_PEERS" | tr ',' '\n' | sed -E 's/^ *| *$//g; s/^/    "/; s/$/",/')"
else
    PEERS_TOML=""  # empty -> DNS-seed discovery (the proven-working path)
fi

if [[ ! -f "$CONFIG_FILE" ]]; then
    log "  writing $CONFIG_FILE"
    cat > "$CONFIG_FILE" <<EOF
# Botho BaaS rig config (generated by rig-bootstrap.sh on $(ts))
# Testnet mining node. Contains the rig wallet mnemonic -> chmod 600.
network_type = "testnet"

[wallet]
mnemonic = "${MNEMONIC}"

[network]
gossip_port = ${GOSSIP_PORT}
rpc_port = ${RPC_PORT}
metrics_port = 19090

# Allow the rig's own HTTPS host + local nginx proxy to call RPC.
cors_origins = ["*"]

# Bootstrap peers. Empty by default: DNS-seed discovery below resolves
# seeds.testnet.botho.io into /ip4/.../p2p/... multiaddrs and connects (the
# proven-working path). Bare /dns4 entries here are NOT supported by the
# transport, so an override via BOOTSTRAP_PEERS must use resolved /ip4 + /p2p.
bootstrap_peers = [
${PEERS_TOML}
]

# DNS-seed discovery is the primary peer source when bootstrap_peers is empty.
[network.dns_seeds]
enabled = true

# Recommended (auto BFT) quorum; mine once at least one peer is connected.
[network.quorum]
mode = "recommended"
min_peers = 1

[minting]
enabled = true
threads = ${MINT_THREADS}

[faucet]
enabled = false
EOF
    chown "$RUN_USER:$RUN_USER" "$CONFIG_FILE"
    chmod 600 "$CONFIG_FILE"
fi

# ===========================================================================
# Step 4: Install + start the botho systemd service (mining)
# ===========================================================================
log "Step 4: installing botho systemd unit"
SERVICE_FILE="/etc/systemd/system/botho.service"
cat > "$SERVICE_FILE" <<EOF
[Unit]
Description=Botho BaaS Rig (Testnet Mining Node)
Documentation=https://github.com/botho-project/botho
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=${RUN_USER}
Group=${RUN_USER}
WorkingDirectory=${RUN_HOME}

# Mining node: join testnet and mint with RandomX.
ExecStart=${BIN_PATH} --testnet run --mint --mint-threads ${MINT_THREADS}

Restart=on-failure
RestartSec=10

LimitNOFILE=65535
LimitNPROC=65535

# Security hardening (mirrors infra/seed/botho-seed.service).
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=${RUN_HOME}/.botho

StandardOutput=journal
StandardError=journal
SyslogIdentifier=botho

[Install]
WantedBy=multi-user.target
EOF

systemctl daemon-reload
systemctl enable botho >/dev/null 2>&1 || true
# Restart to pick up any new binary/config on a re-run.
systemctl restart botho
log "  botho.service started"

# ===========================================================================
# Step 5: nginx + TLS + /rpc reverse proxy for the rig hostname
# ===========================================================================
if [[ -z "$RIG_HOSTNAME" ]]; then
    log "Step 5: RIG_HOSTNAME unset -> skipping public nginx/TLS (node still serves RPC on localhost:${RPC_PORT})"
else
    log "Step 5: configuring nginx (+TLS mode=$TLS_MODE) for $RIG_HOSTNAME"
    install -d -m 0755 /var/www/certbot
    NGINX_SITE="/etc/nginx/sites-available/${RIG_HOSTNAME}"

    write_http_only_site() {
        # Minimal HTTP server: ACME challenge + /rpc proxy. Used before certs
        # exist (webroot mode) and as the whole config in TLS_MODE=skip.
        cat > "$NGINX_SITE" <<EOF
map \$http_upgrade \$connection_upgrade { default upgrade; '' close; }
server {
    listen 80;
    listen [::]:80;
    server_name ${RIG_HOSTNAME};

    location /.well-known/acme-challenge/ { root /var/www/certbot; }
    location /health { access_log off; add_header Content-Type text/plain; return 200 "OK"; }

    location /rpc {
        limit_except POST OPTIONS { deny all; }
        if (\$request_method = 'OPTIONS') {
            add_header 'Access-Control-Allow-Origin' '*' always;
            add_header 'Access-Control-Allow-Methods' 'POST, OPTIONS' always;
            add_header 'Access-Control-Allow-Headers' 'Content-Type' always;
            add_header 'Content-Length' 0; add_header 'Content-Type' 'text/plain';
            return 204;
        }
        proxy_pass http://127.0.0.1:${RPC_PORT};
        proxy_http_version 1.1;
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
        proxy_hide_header 'Access-Control-Allow-Origin';
        proxy_hide_header 'Vary';
        add_header 'Access-Control-Allow-Origin' '*' always;
        client_max_body_size 64k;
    }
}
EOF
    }

    write_tls_site() {
        # Full HTTPS site mirroring infra/seed/seed-nginx.conf: HTTP->HTTPS
        # redirect, TLS, /rpc and /rpc/ws proxy with CORS de-duplication.
        cat > "$NGINX_SITE" <<EOF
map \$http_upgrade \$connection_upgrade { default upgrade; '' close; }

server {
    listen 80;
    listen [::]:80;
    server_name ${RIG_HOSTNAME};
    location /.well-known/acme-challenge/ { root /var/www/certbot; }
    location / { return 301 https://\$host\$request_uri; }
}

server {
    listen 443 ssl http2;
    listen [::]:443 ssl http2;
    server_name ${RIG_HOSTNAME};

    ssl_certificate /etc/letsencrypt/live/${RIG_HOSTNAME}/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/${RIG_HOSTNAME}/privkey.pem;
    ssl_protocols TLSv1.2 TLSv1.3;
    ssl_prefer_server_ciphers on;
    ssl_session_cache shared:SSL:10m;
    ssl_session_timeout 1d;

    add_header X-Frame-Options "SAMEORIGIN" always;
    add_header X-Content-Type-Options "nosniff" always;
    add_header Referrer-Policy "strict-origin-when-cross-origin" always;

    location /health { access_log off; add_header Content-Type text/plain; return 200 "OK"; }

    location = /rpc/ws {
        if (\$request_method = 'OPTIONS') {
            add_header 'Access-Control-Allow-Origin' '*' always;
            add_header 'Access-Control-Allow-Methods' 'GET, OPTIONS' always;
            add_header 'Access-Control-Allow-Headers' 'Content-Type, Upgrade, Connection' always;
            add_header 'Content-Length' 0; add_header 'Content-Type' 'text/plain';
            return 204;
        }
        proxy_pass http://127.0.0.1:${RPC_PORT}/ws;
        proxy_http_version 1.1;
        proxy_set_header Upgrade \$http_upgrade;
        proxy_set_header Connection \$connection_upgrade;
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
        proxy_hide_header 'Access-Control-Allow-Origin';
        proxy_connect_timeout 10s;
        proxy_send_timeout 3600s;
        proxy_read_timeout 3600s;
        add_header 'Access-Control-Allow-Origin' '*' always;
    }

    location /rpc {
        limit_except POST OPTIONS { deny all; }
        if (\$request_method = 'OPTIONS') {
            add_header 'Access-Control-Allow-Origin' '*' always;
            add_header 'Access-Control-Allow-Methods' 'POST, OPTIONS' always;
            add_header 'Access-Control-Allow-Headers' 'Content-Type' always;
            add_header 'Content-Length' 0; add_header 'Content-Type' 'text/plain';
            return 204;
        }
        proxy_pass http://127.0.0.1:${RPC_PORT};
        proxy_http_version 1.1;
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto \$scheme;
        proxy_hide_header 'Access-Control-Allow-Origin';
        proxy_hide_header 'Access-Control-Allow-Methods';
        proxy_hide_header 'Access-Control-Allow-Headers';
        proxy_hide_header 'Vary';
        proxy_connect_timeout 10s;
        proxy_send_timeout 30s;
        proxy_read_timeout 30s;
        add_header 'Access-Control-Allow-Origin' '*' always;
        add_header 'Access-Control-Allow-Methods' 'POST, OPTIONS' always;
        add_header 'Access-Control-Allow-Headers' 'Content-Type' always;
        client_max_body_size 64k;
    }
}
EOF
    }

    # Remove the default site so server_name matching is unambiguous.
    rm -f /etc/nginx/sites-enabled/default

    have_cert() { [[ -f "/etc/letsencrypt/live/${RIG_HOSTNAME}/fullchain.pem" ]]; }

    case "$TLS_MODE" in
        skip)
            write_http_only_site
            ln -sf "$NGINX_SITE" "/etc/nginx/sites-enabled/${RIG_HOSTNAME}"
            nginx -t && systemctl reload nginx
            log "  nginx HTTP-only (/rpc) ready (TLS skipped)"
            ;;
        standalone)
            if ! have_cert; then
                systemctl stop nginx || true
                certbot certonly --standalone -n --agree-tos -m "$CERTBOT_EMAIL" \
                    -d "$RIG_HOSTNAME" || log "  WARN: certbot --standalone failed (DNS not pointed yet?)"
            fi
            if have_cert; then write_tls_site; else write_http_only_site; fi
            ln -sf "$NGINX_SITE" "/etc/nginx/sites-enabled/${RIG_HOSTNAME}"
            nginx -t && systemctl restart nginx
            log "  nginx ready ($(have_cert && echo HTTPS || echo HTTP-only))"
            ;;
        webroot|*)
            # Bring up HTTP first so the ACME webroot challenge can be served,
            # then obtain a cert and switch to the full TLS site.
            write_http_only_site
            ln -sf "$NGINX_SITE" "/etc/nginx/sites-enabled/${RIG_HOSTNAME}"
            nginx -t && systemctl reload nginx
            if ! have_cert; then
                certbot certonly --webroot -w /var/www/certbot -n --agree-tos \
                    -m "$CERTBOT_EMAIL" -d "$RIG_HOSTNAME" \
                    || log "  WARN: certbot webroot failed (is DNS for $RIG_HOSTNAME pointed at this host yet?). Serving HTTP-only for now; re-run this script after DNS propagates."
            fi
            if have_cert; then
                write_tls_site
                nginx -t && systemctl reload nginx
                log "  nginx HTTPS /rpc ready for https://${RIG_HOSTNAME}/rpc"
            else
                log "  nginx HTTP-only /rpc ready (no cert yet)"
            fi
            ;;
    esac
fi

# ===========================================================================
# Step 6: Emit rig info + install a `rig-status` read-back helper
# ===========================================================================
log "Step 6: writing rig-info + installing rig-status helper"

cat > /usr/local/bin/rig-status <<EOF
#!/usr/bin/env bash
# Read back this rig's node status + RPC URL.
set -euo pipefail
echo "botho.service: \$(systemctl is-active botho 2>/dev/null || echo inactive)"
RESP="\$(curl -s -m 8 -X POST http://localhost:${RPC_PORT} -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}' || true)"
if command -v jq >/dev/null 2>&1 && [[ -n "\$RESP" ]]; then
    echo "\$RESP" | jq '{network:.result.network, height:.result.chainHeight, peers:.result.peerCount, synced:.result.synced, syncStatus:.result.syncStatus, mintingActive:.result.mintingActive}'
else
    echo "\$RESP"
fi
EOF
chmod +x /usr/local/bin/rig-status

# Best-effort metadata reads. These MUST NOT abort the script (set -e/pipefail),
# so each is guarded and failures degrade to "unknown".
PUBLIC_IP="unknown"
BIN_VERSION="unknown ${BIN_ARCH}"
set +e
# IMDSv2: fetch a token first (the Ubuntu arm64 AMI defaults to IMDSv2).
IMDS_TOKEN="$(curl -s -m 3 -X PUT "http://169.254.169.254/latest/api/token" \
    -H "X-aws-ec2-metadata-token-ttl-seconds: 60" 2>/dev/null)"
IP_TMP="$(curl -s -m 3 -H "X-aws-ec2-metadata-token: ${IMDS_TOKEN}" \
    http://169.254.169.254/latest/meta-data/public-ipv4 2>/dev/null)"
[[ -n "$IP_TMP" ]] && PUBLIC_IP="$IP_TMP"
# Resolve the running node's version from RPC (binary has no --version flag).
VER_RESP="$(curl -s -m 5 -X POST "http://localhost:${RPC_PORT}" \
    -H 'Content-Type: application/json' \
    -d '{"jsonrpc":"2.0","method":"node_getStatus","params":{},"id":1}' 2>/dev/null)"
VER_TMP="$(printf '%s' "$VER_RESP" | jq -r '.result.nodeVersion // empty' 2>/dev/null)"
[[ -n "$VER_TMP" ]] && BIN_VERSION="${VER_TMP} ${BIN_ARCH}"
set -e

if [[ -n "$RIG_HOSTNAME" ]]; then
    RPC_URL="https://${RIG_HOSTNAME}/rpc  (or http://${RIG_HOSTNAME}/rpc until TLS issued)"
else
    RPC_URL="http://localhost:${RPC_PORT}  (no public hostname configured)"
fi

cat > "${RUN_HOME}/rig-info.txt" <<EOF
# Botho rig provisioned by rig-bootstrap.sh on $(ts)
network        = ${NETWORK}
rig_hostname   = ${RIG_HOSTNAME:-<none>}
public_ip      = ${PUBLIC_IP}
binary_version = ${BIN_VERSION}
rpc_url        = ${RPC_URL}
local_rpc      = http://localhost:${RPC_PORT}
config         = ${CONFIG_FILE}   (mnemonic inside, chmod 600)
service        = systemctl status botho
logs           = journalctl -u botho -f
status         = sudo rig-status
EOF
chown "$RUN_USER:$RUN_USER" "${RUN_HOME}/rig-info.txt"

# Give the node a moment, then log a first status (best effort).
sleep 5
log "Initial node status:"
/usr/local/bin/rig-status 2>&1 | sed 's/^/    /' || true

log "=== Botho rig bootstrap complete ==="
log "Read back any time: sudo rig-status   |   cat ${RUN_HOME}/rig-info.txt"

# ---------------------------------------------------------------------------
# BINARY SOURCE NOTE (for #458)
# ---------------------------------------------------------------------------
# As of this writing the latest GitHub release (v0.2.0) has NO published
# binary asset, so this bootstrap cannot fetch one from a public URL on its
# own. The provisioner (#458) MUST publish the current linux-aarch64 `botho`
# release artifact (e.g. upload to S3/R2 or attach it to the GitHub release)
# and pass its URL as BOTHO_BINARY_URL. See infra/baas/README.md for the
# recommended distribution flow and the interim "copy from a live seed"
# stand-in used during verification.
