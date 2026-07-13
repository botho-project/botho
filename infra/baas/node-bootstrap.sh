#!/usr/bin/env bash
#
# Botho BaaS Node Bootstrap (cloud-init / EC2 user-data)
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
#     sudo NODE_ID=demo REGION=us-west-2 TIER=t4g.medium ./node-bootstrap.sh
#
# (with no BOTHO_BINARY_URL, the latest GitHub release's linux-aarch64 tarball
# is resolved and checksum-pinned automatically), or with an explicit pin:
#
#     sudo NODE_ID=demo REGION=us-west-2 TIER=t4g.medium \
#          BOTHO_BINARY_URL=https://github.com/botho-project/botho/releases/download/v0.3.0/botho-v0.3.0-linux-aarch64.tar.gz \
#          BOTHO_BINARY_SHA256=<'botho' line from checksums-linux-aarch64.txt> \
#          ./node-bootstrap.sh
#
# (NODE_ID=demo derives NODE_HOSTNAME=node-demo.testnet.botho.io.)
#
# Re-running is safe (idempotent): each step checks current state first.
#
# Flags:
#   --tls-retry-only   internal: run only the certbot-until-DNS retry step.
#                      Invoked by the botho-tls-retry systemd timer; not for
#                      manual use.
#   --dry-run          local, non-EC2 smoke test of the two #807 additions
#                      (DNS-seed -> bootstrap_peers resolution + systemd
#                      unit-file rendering). No apt/certbot/systemd side effects.
#
# ---------------------------------------------------------------------------
# PARAMETERS (environment variables / user-data exports)
# ---------------------------------------------------------------------------
#   BOTHO_BINARY_URL   (optional) URL to the prebuilt linux-aarch64 botho
#                      build. Accepts EITHER the published GitHub release
#                      tarball (`botho-vX.Y.Z-linux-aarch64.tar.gz`, the
#                      canonical source since v0.3.0 — the `botho` member is
#                      extracted and installed) OR a bare aarch64 `botho`
#                      binary (e.g. an S3/R2 mirror object; legacy path, still
#                      supported). The node is arm64, so it DOWNLOADS a prebuilt
#                      binary rather than building from source on the box.
#                      When unset: an already-installed /usr/local/bin/botho is
#                      reused (idempotent re-run); otherwise the latest GitHub
#                      release's linux-aarch64 tarball is resolved via the
#                      GitHub API and used automatically. See BINARY SOURCE
#                      note at the bottom of this file and infra/baas/README.md.
#   BOTHO_BINARY_SHA256 (optional) expected sha256 of the `botho` BINARY —
#                      verified if set. In tarball mode this is compared against
#                      the EXTRACTED `botho` (the release publishes per-binary
#                      digests: pass the `botho` line of the release asset
#                      `checksums-linux-aarch64.txt`; the tarball's own digest
#                      is published nowhere). In bare-binary mode the downloaded
#                      file itself is verified. Do NOT take digests from
#                      SHA256SUMS.txt — it concatenates all platforms without
#                      labels and lists `botho` multiple times. When both this
#                      and BOTHO_BINARY_URL are unset (latest-release mode), the
#                      digest is fetched from the same release's
#                      checksums-linux-aarch64.txt and pinned automatically.
#   BOTHO_REPO         (optional, default "botho-project/botho") GitHub
#                      owner/repo used for latest-release resolution.
#   NODE_ID             (optional) short opaque node identifier (e.g. abc123),
#                      assigned by the provisioner / Stripe subscription mapping.
#                      When set and NODE_HOSTNAME is unset, the public hostname is
#                      derived as node-<NODE_ID>.<NODE_DOMAIN>. Recorded in
#                      node-info.txt for control-plane traceability.
#   NODE_DOMAIN         (optional, default "testnet.botho.io") the zone under
#                      which node-<NODE_ID> hostnames live; combined with NODE_ID to
#                      derive NODE_HOSTNAME when the latter is not given directly.
#   NODE_HOSTNAME       (optional) public hostname for this node, e.g.
#                      node-abc123.testnet.botho.io. Takes precedence over
#                      NODE_ID/NODE_DOMAIN. The provisioner pre-creates the DNS A
#                      record -> this instance's public IP BEFORE boot. If
#                      neither NODE_HOSTNAME nor NODE_ID is set, TLS/nginx public
#                      setup is skipped and the node still serves RPC on
#                      localhost:17101.
#   REGION             (optional) AWS region the node was launched in (e.g.
#                      us-west-2). Informational here — the instance is already
#                      in its region by the time user-data runs; the provisioner
#                      (#458 P6.2) picks the region at run-instances time.
#                      Recorded in node-info.txt.
#   TIER               (optional, default "t4g.medium") instance type / tier the
#                      provisioner launched. Informational; recorded in
#                      node-info.txt. The MVP is t4g.medium-only (#458 §5).
#   NETWORK            (optional, default "testnet"). Only "testnet" is
#                      supported by this slice.
#   BOOTSTRAP_PEERS    (optional) comma-separated RESOLVED libp2p multiaddrs
#                      (/ip4/<ip>/tcp/<port>/p2p/<peer_id>) to use as
#                      bootstrap_peers verbatim. When unset, the script resolves
#                      SEED_DOMAIN's TXT records into /ip4 peers itself (Fix 2,
#                      #807); on empty/failed resolution it falls back to an
#                      empty list + DNS-seed discovery with a loud log.
#   SEED_DOMAIN        (optional, default "seeds.<NODE_DOMAIN>") DNS-seed domain
#                      whose PEER_ID@host:port TXT records are resolved into
#                      bootstrap_peers at config-write time (fresh provisions).
#   MINT_THREADS       (optional, default 1) RandomX minting threads. t4g.medium
#                      has 2 vCPU / ~4GB; 1 thread leaves headroom.
#   CERTBOT_EMAIL      (optional) email for Let's Encrypt registration.
#   TLS_MODE           (optional) "webroot" (default, needs nginx+DNS),
#                      "standalone" (certbot --standalone, stops nginx briefly),
#                      or "skip" (no certbot; HTTP-only nginx for local test).
#   NODE_WALLET_MNEMONIC (optional) bring-your-own 24-word mnemonic. Default:
#                      generate a fresh per-node mnemonic. (#458 will decide
#                      bring-your-own vs generated; the param exists already.)
#   BIP39_WORDLIST_URL (optional) URL to the BIP39 English wordlist (2048 words,
#                      one per line) used to generate the node mnemonic. Default:
#                      the canonical bitcoin/bips raw URL. A local copy next to
#                      this script (or at /usr/local/share/botho/) is used first
#                      if present, so user-data stays small.
#
# ---------------------------------------------------------------------------
# OUTPUTS
# ---------------------------------------------------------------------------
#   /var/log/botho-node-bootstrap.log   full provisioning log
#   /home/ubuntu/.botho/testnet/config.toml   node config (mnemonic, chmod 600)
#   /home/ubuntu/node-info.txt          machine-readable summary (RPC URL, peer
#                                      id, mnemonic location, status command)
#   systemd unit `botho` running `botho --testnet run --mint`
#   systemd `botho-tls-retry.timer`/.service (only when the first inline certbot
#     attempt fails because DNS is not yet pointed here) — retries certbot until
#     DNS resolves, then self-disables. Re-invokes this script with
#     --tls-retry-only from /usr/local/sbin/botho-node-bootstrap.sh.
#   Read back any time with:  sudo /usr/local/bin/node-status   (installed here)
#
set -euo pipefail

# ---------------------------------------------------------------------------
# Logging: tee everything to a persistent log AND the cloud-init console.
# ---------------------------------------------------------------------------
LOG_FILE="/var/log/botho-node-bootstrap.log"
# Degrade to a user-writable log if /var/log is not writable (e.g. a local
# --dry-run as a non-root user); production/cloud-init runs as root and uses
# the canonical path.
if ! { : >> "$LOG_FILE"; } 2>/dev/null; then
    LOG_FILE="${TMPDIR:-/tmp}/botho-node-bootstrap.log"
fi
exec > >(tee -a "$LOG_FILE") 2>&1

ts()   { date -u +"%Y-%m-%dT%H:%M:%SZ"; }
log()  { echo "[$(ts)] [node-bootstrap] $*"; }
fail() { echo "[$(ts)] [node-bootstrap] FATAL: $*" >&2; exit 1; }

log "=== Botho node bootstrap starting ==="

# ---------------------------------------------------------------------------
# Parameters & defaults
# ---------------------------------------------------------------------------
NETWORK="${NETWORK:-testnet}"
NODE_ID="${NODE_ID:-}"
NODE_DOMAIN="${NODE_DOMAIN:-testnet.botho.io}"
NODE_HOSTNAME="${NODE_HOSTNAME:-}"
REGION="${REGION:-}"
TIER="${TIER:-t4g.medium}"
BOTHO_BINARY_URL="${BOTHO_BINARY_URL:-}"
BOTHO_BINARY_SHA256="${BOTHO_BINARY_SHA256:-}"
BOTHO_REPO="${BOTHO_REPO:-botho-project/botho}"
BOOTSTRAP_PEERS="${BOOTSTRAP_PEERS:-}"
MINT_THREADS="${MINT_THREADS:-1}"
CERTBOT_EMAIL="${CERTBOT_EMAIL:-admin@botho.io}"
TLS_MODE="${TLS_MODE:-webroot}"
NODE_WALLET_MNEMONIC="${NODE_WALLET_MNEMONIC:-}"
BIP39_WORDLIST_URL="${BIP39_WORDLIST_URL:-https://raw.githubusercontent.com/bitcoin/bips/master/bip-0039/english.txt}"
# DNS-seed domain resolved into bootstrap_peers at config-write time (Fix 2 /
# issue #807). Testnet only in this slice, matching the NETWORK=testnet guard.
SEED_DOMAIN="${SEED_DOMAIN:-seeds.${NODE_DOMAIN}}"

# Mode/flag handling. The script has three entry modes:
#   (default)         full first-boot provisioning (cloud-init user-data).
#   --tls-retry-only  invoked ONLY by the botho-tls-retry systemd timer (Fix 1 /
#                     issue #807): verify DNS points here, then try to issue the
#                     TLS cert and swap nginx to the HTTPS site, self-disabling
#                     the timer once a cert exists (or the retry cap elapses).
#   --dry-run         local, non-EC2 function test of the two new pieces (seed
#                     resolution + unit-file rendering). No apt/certbot/systemd.
MODE="full"
DRY_RUN=0
for arg in "$@"; do
    case "$arg" in
        --tls-retry-only) MODE="tls-retry" ;;
        --dry-run)        DRY_RUN=1 ;;
        *) fail "unknown argument: $arg (supported: --tls-retry-only, --dry-run)" ;;
    esac
done

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
# The run-user guard is a production/EC2 precondition; skip it in --dry-run so
# the new-behavior smoke test works on a dev laptop without an 'ubuntu' user.
if [[ "$DRY_RUN" -ne 1 ]]; then
    id "$RUN_USER" >/dev/null 2>&1 || fail "Expected user '$RUN_USER' to exist (Ubuntu arm64 AMI)."
fi

# Derive the public hostname from NODE_ID when NODE_HOSTNAME was not given
# directly. The provisioner (#458 P6.2) assigns NODE_ID per subscription and
# creates the DNS A record for node-<NODE_ID>.<NODE_DOMAIN> before boot.
if [[ -z "$NODE_HOSTNAME" && -n "$NODE_ID" ]]; then
    # Allow NODE_ID to be either the bare id (abc123) or a full "node-abc123".
    case "$NODE_ID" in
        node-*) NODE_HOSTNAME="${NODE_ID}.${NODE_DOMAIN}" ;;
        *)     NODE_HOSTNAME="node-${NODE_ID}.${NODE_DOMAIN}" ;;
    esac
    log "Derived NODE_HOSTNAME='$NODE_HOSTNAME' from NODE_ID='$NODE_ID' NODE_DOMAIN='$NODE_DOMAIN'"
fi

log "Params: NETWORK=$NETWORK NODE_ID='${NODE_ID:-<none>}' NODE_HOSTNAME='${NODE_HOSTNAME:-<none>}' REGION='${REGION:-<unset>}' TIER=$TIER TLS_MODE=$TLS_MODE MINT_THREADS=$MINT_THREADS"
log "Binary source: ${BOTHO_BINARY_URL:-<unset: reuse existing $BIN_PATH, else latest GitHub release>}"

# Path to the resolved copy of this script that the TLS-retry timer re-invokes.
# cloud-init runs the original user-data from a transient path, so we install a
# stable copy here (Step 5) and point the systemd unit at it.
SELF_INSTALL_PATH="/usr/local/sbin/botho-node-bootstrap.sh"
# systemd units + retry bookkeeping for the certbot-until-DNS retry (Fix 1).
TLS_RETRY_SERVICE="/etc/systemd/system/botho-tls-retry.service"
TLS_RETRY_TIMER="/etc/systemd/system/botho-tls-retry.timer"
TLS_RETRY_STAMP="/var/lib/botho/tls-retry-first-attempt.epoch"
TLS_RETRY_CAP_SECONDS=$((2 * 60 * 60))   # give up after ~2h of retries

# ===========================================================================
# Shared helpers (used by both full provisioning AND --tls-retry-only mode)
# ===========================================================================
NGINX_SITE="/etc/nginx/sites-available/${NODE_HOSTNAME:-_unset}"

have_cert() { [[ -f "/etc/letsencrypt/live/${NODE_HOSTNAME}/fullchain.pem" ]]; }

write_http_only_site() {
    # Minimal HTTP server: ACME challenge + /rpc proxy. Used before certs
    # exist (webroot mode) and as the whole config in TLS_MODE=skip.
    cat > "$NGINX_SITE" <<EOF
map \$http_upgrade \$connection_upgrade { default upgrade; '' close; }
server {
    listen 80;
    listen [::]:80;
    server_name ${NODE_HOSTNAME};

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
    server_name ${NODE_HOSTNAME};
    location /.well-known/acme-challenge/ { root /var/www/certbot; }
    location / { return 301 https://\$host\$request_uri; }
}

server {
    listen 443 ssl http2;
    listen [::]:443 ssl http2;
    server_name ${NODE_HOSTNAME};

    ssl_certificate /etc/letsencrypt/live/${NODE_HOSTNAME}/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/${NODE_HOSTNAME}/privkey.pem;
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

# Resolve this instance's own public IPv4 via IMDSv2 (same token pattern as
# Step 6). Prints the IP or nothing.
imds_public_ipv4() {
    local token ip
    token="$(curl -s -m 3 -X PUT "http://169.254.169.254/latest/api/token" \
        -H "X-aws-ec2-metadata-token-ttl-seconds: 60" 2>/dev/null)"
    ip="$(curl -s -m 3 -H "X-aws-ec2-metadata-token: ${token}" \
        http://169.254.169.254/latest/meta-data/public-ipv4 2>/dev/null)"
    printf '%s' "$ip"
}

# Resolve a hostname to its first A record. Prefers dig, falls back to getent
# (which does A/AAAA lookups even without dnsutils installed).
resolve_a() {
    local host="$1" ip=""
    if command -v dig >/dev/null 2>&1; then
        ip="$(dig +short A "$host" 2>/dev/null | grep -E '^[0-9]+\.' | head -1)"
    fi
    if [[ -z "$ip" ]]; then
        ip="$(getent ahostsv4 "$host" 2>/dev/null | awk '{print $1; exit}')"
    fi
    printf '%s' "$ip"
}

# True iff the node hostname's A record currently resolves to THIS instance's
# public IP — the precondition for a certbot attempt (so the retry timer does
# not burn Let's Encrypt rate limits while DNS is still absent/stale).
dns_points_here() {
    local public resolved
    public="$(imds_public_ipv4)"
    resolved="$(resolve_a "$NODE_HOSTNAME")"
    [[ -n "$public" && -n "$resolved" && "$public" == "$resolved" ]]
}

# Fix 2: resolve the DNS-seed TXT records (peerid@host:port) into fully
# resolved /ip4/<ip>/tcp/<port>/p2p/<peerid> multiaddrs and echo them as TOML
# array lines. Empty output => caller falls back to an empty bootstrap_peers
# list (today's behavior) with a loud log. This mirrors the node's own
# parse_seed_record format (botho/src/network/dns_seeds.rs) but emits the
# transport-accepted /ip4 form directly rather than the /dns4 form the node's
# parser produces for hostname seeds (which the libp2p transport rejects).
resolve_seed_peers() {
    local domain="$1" txt line record host_port peer_id host port ip resolved_ip
    if ! command -v dig >/dev/null 2>&1; then
        log "  WARN: dig not available; cannot resolve TXT seeds for $domain" >&2
        return 0
    fi
    txt="$(dig +short TXT "$domain" 2>/dev/null)"
    [[ -n "$txt" ]] || return 0
    while IFS= read -r line; do
        # dig prints TXT values quoted; strip surrounding quotes.
        record="${line%\"}"; record="${record#\"}"
        [[ -n "$record" ]] || continue
        # Format: PEER_ID@host:port
        case "$record" in
            *@*:*) : ;;
            *) continue ;;
        esac
        peer_id="${record%%@*}"
        host_port="${record#*@}"
        host="${host_port%:*}"
        port="${host_port##*:}"
        [[ -n "$peer_id" && -n "$host" && -n "$port" ]] || continue
        # Host may already be an IP; resolve_a passes IPs through via getent.
        if [[ "$host" =~ ^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
            ip="$host"
        else
            ip="$(resolve_a "$host")"
        fi
        [[ -n "$ip" ]] || { log "  WARN: could not A-resolve seed host '$host'" >&2; continue; }
        resolved_ip="$ip"
        printf '    "/ip4/%s/tcp/%s/p2p/%s",\n' "$resolved_ip" "$port" "$peer_id"
    done <<< "$txt"
}

# Fix 1: single attempt to acquire a webroot cert and swap nginx to the HTTPS
# site. Assumes nginx is already serving the HTTP-only site (ACME webroot at
# /var/www/certbot). Returns 0 iff a cert exists afterwards. Called by BOTH the
# inline first attempt in Step 5 and the --tls-retry-only timer path, so the
# certbot invocation lives in exactly one place.
try_issue_tls_cert() {
    if have_cert; then
        return 0
    fi
    if ! dns_points_here; then
        log "  TLS: DNS for $NODE_HOSTNAME does not resolve to this instance yet; skipping certbot this attempt"
        return 1
    fi
    log "  TLS: DNS points here; requesting Let's Encrypt cert for $NODE_HOSTNAME (webroot)"
    certbot certonly --webroot -w /var/www/certbot -n --agree-tos \
        -m "$CERTBOT_EMAIL" -d "$NODE_HOSTNAME" \
        || log "  WARN: certbot webroot failed for $NODE_HOSTNAME (will retry via timer)"
    if have_cert; then
        write_tls_site
        nginx -t && systemctl reload nginx
        log "  TLS: HTTPS /rpc ready for https://${NODE_HOSTNAME}/rpc"
        return 0
    fi
    return 1
}

# Install (idempotently) the systemd oneshot+timer that re-invokes this script
# with --tls-retry-only until a cert is issued or the retry cap elapses. Unit
# content is deterministic, so re-running the bootstrap by hand just rewrites
# identical files (no duplicate/competing timers).
install_tls_retry_timer() {
    install -d -m 0755 "$(dirname "$SELF_INSTALL_PATH")"
    install -m 0755 "$SCRIPT_SELF" "$SELF_INSTALL_PATH"
    install -d -m 0755 "$(dirname "$TLS_RETRY_STAMP")"

    cat > "$TLS_RETRY_SERVICE" <<EOF
[Unit]
Description=Botho BaaS node TLS cert retry (certbot until DNS resolves)
Documentation=https://github.com/botho-project/botho/blob/main/infra/baas/README.md
After=network-online.target nginx.service
Wants=network-online.target

[Service]
Type=oneshot
Environment=NODE_HOSTNAME=${NODE_HOSTNAME}
Environment=CERTBOT_EMAIL=${CERTBOT_EMAIL}
Environment=NETWORK=${NETWORK}
ExecStart=${SELF_INSTALL_PATH} --tls-retry-only
StandardOutput=journal
StandardError=journal
SyslogIdentifier=botho-tls-retry
EOF

    cat > "$TLS_RETRY_TIMER" <<EOF
[Unit]
Description=Botho BaaS node TLS cert retry timer
Documentation=https://github.com/botho-project/botho/blob/main/infra/baas/README.md

[Timer]
# Retry shortly after boot, then every 5 minutes, until the oneshot
# self-disables the timer (cert issued) or the ~2h cap is reached.
OnBootSec=1min
OnUnitActiveSec=5min
# Survive reboots that happen before DNS propagates.
Persistent=true
AccuracySec=30s

[Install]
WantedBy=timers.target
EOF

    systemctl daemon-reload
    systemctl enable --now botho-tls-retry.timer >/dev/null 2>&1 \
        || log "  WARN: could not enable botho-tls-retry.timer"
    log "  TLS retry timer installed (retries certbot until DNS resolves, cap ~2h)"
}

# Stop the retry timer for good (cert issued, or retry cap exceeded).
disable_tls_retry_timer() {
    systemctl disable --now botho-tls-retry.timer >/dev/null 2>&1 || true
}

# --tls-retry-only entry point: one timer tick. Verify DNS, try the cert, and
# self-disable the timer on success or once past the ~2h cap.
run_tls_retry_only() {
    [[ -n "$NODE_HOSTNAME" ]] || fail "--tls-retry-only requires NODE_HOSTNAME"
    log "=== Botho TLS retry tick for $NODE_HOSTNAME ==="

    if have_cert; then
        log "  cert already present; ensuring TLS site is active and disabling retry timer"
        write_tls_site
        nginx -t && systemctl reload nginx || true
        disable_tls_retry_timer
        return 0
    fi

    # Retry cap bookkeeping: stamp the first attempt, give up after the cap.
    install -d -m 0755 "$(dirname "$TLS_RETRY_STAMP")"
    local now first
    now="$(date +%s)"
    if [[ -f "$TLS_RETRY_STAMP" ]]; then
        first="$(cat "$TLS_RETRY_STAMP" 2>/dev/null || echo "$now")"
    else
        first="$now"
        echo "$first" > "$TLS_RETRY_STAMP"
    fi
    if [[ "$first" =~ ^[0-9]+$ ]] && (( now - first > TLS_RETRY_CAP_SECONDS )); then
        log "  ERROR: TLS retry cap (~2h) exceeded for $NODE_HOSTNAME with no cert; giving up and disabling timer. Investigate DNS for $NODE_HOSTNAME, then re-run node-bootstrap.sh."
        disable_tls_retry_timer
        return 0
    fi

    if try_issue_tls_cert; then
        log "  cert issued; disabling retry timer"
        disable_tls_retry_timer
    else
        log "  no cert yet ($(( (now - first) / 60 ))min elapsed of ~120min cap); timer will fire again"
    fi
    return 0
}

# --dry-run entry point: exercise the two new pieces locally (no EC2, no root,
# no apt/certbot/systemd). Renders the seed-peer resolution and the systemd
# unit files to stdout / a temp dir so they can be eyeballed and lint-checked.
run_dry_run() {
    log "=== DRY RUN: node-bootstrap.sh new-behavior smoke ==="
    log "SEED_DOMAIN=$SEED_DOMAIN NODE_HOSTNAME='${NODE_HOSTNAME:-<none>}'"
    log "resolve_seed_peers($SEED_DOMAIN) =>"
    local peers
    peers="$(resolve_seed_peers "$SEED_DOMAIN" || true)"
    if [[ -n "$peers" ]]; then
        printf '%s\n' "$peers"
    else
        log "  (empty — would fall back to bootstrap_peers = [] + DNS-seed discovery)"
    fi
    log "TLS retry timer would be installed at:"
    log "  service: $TLS_RETRY_SERVICE  (ExecStart=$SELF_INSTALL_PATH --tls-retry-only)"
    log "  timer:   $TLS_RETRY_TIMER    (OnBootSec=1min OnUnitActiveSec=5min cap=${TLS_RETRY_CAP_SECONDS}s)"
    log "=== DRY RUN complete ==="
    exit 0
}

# Resolve the path to THIS script so the timer can re-invoke it. In cloud-init,
# $0 is a transient user-data path; we copy it to SELF_INSTALL_PATH in Step 5.
SCRIPT_SELF="$(readlink -f "$0" 2>/dev/null || echo "$0")"

# Mode dispatch (after all helpers are defined).
if [[ "$DRY_RUN" -eq 1 ]]; then
    run_dry_run
fi
if [[ "$MODE" == "tls-retry" ]]; then
    run_tls_retry_only
    exit 0
fi

# ===========================================================================
# Step 1: Install dependencies (idempotent)
# ===========================================================================
log "Step 1: installing system dependencies"
export DEBIAN_FRONTEND=noninteractive
NEEDED_PKGS=(nginx certbot python3-certbot-nginx curl ca-certificates jq dnsutils)
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
# builds take ~20+ min and can OOM RandomX-linked crates). Source resolution
# order (see BINARY SOURCE NOTE at the bottom of this file):
#   1. explicit BOTHO_BINARY_URL (release tarball or bare-binary mirror),
#   2. existing $BIN_PATH (idempotent re-run — never re-downloads),
#   3. the latest GitHub release's linux-aarch64 tarball, checksum-pinned from
#      the same release's checksums-linux-aarch64.txt (canonical since v0.3.0;
#      release assets preferred per #638).
log "Step 2: obtaining botho binary"
install_binary() {
    # $1 = URL to EITHER the release tarball botho-vX.Y.Z-linux-aarch64.tar.gz
    # (gzip; top-level `botho` member, mode 0644 inside the archive) OR a bare
    # aarch64 `botho` binary (legacy S3/R2 mirror path). Either way the
    # resulting binary is installed 0755 at $BIN_PATH.
    local url="$1" tmp candidate extract_dir=""
    tmp="$(mktemp /tmp/botho.XXXXXX)"
    log "  downloading $url"
    curl -fSL --retry 5 --retry-delay 5 -o "$tmp" "$url" \
        || fail "failed to download botho binary from $url"
    candidate="$tmp"
    if file "$tmp" | grep -qi "gzip compressed"; then
        log "  gzip tarball detected; extracting 'botho' member"
        extract_dir="$(mktemp -d /tmp/botho-extract.XXXXXX)"
        tar -xzf "$tmp" -C "$extract_dir" \
            || fail "failed to extract release tarball downloaded from $url"
        candidate="$extract_dir/botho"
        [[ -f "$candidate" ]] \
            || fail "release tarball from $url has no top-level 'botho' member"
    fi
    if [[ -n "$BOTHO_BINARY_SHA256" ]]; then
        # In tarball mode this verifies the EXTRACTED `botho` binary: the
        # release publishes per-binary digests (checksums-linux-aarch64.txt),
        # not a tarball digest. In bare-binary mode it verifies the download.
        local got
        got="$(sha256sum "$candidate" | awk '{print $1}')"
        [[ "$got" == "$BOTHO_BINARY_SHA256" ]] \
            || fail "binary sha256 mismatch: got $got expected $BOTHO_BINARY_SHA256"
        log "  sha256 verified: $got"
    fi
    file "$candidate" | grep -q "aarch64" \
        || fail "botho binary is not aarch64: $(file "$candidate")"
    install -m 0755 "$candidate" "$BIN_PATH"
    rm -f "$tmp"
    if [[ -n "$extract_dir" ]]; then rm -rf "$extract_dir"; fi
}

if [[ -n "$BOTHO_BINARY_URL" ]]; then
    install_binary "$BOTHO_BINARY_URL"
elif [[ -x "$BIN_PATH" ]]; then
    log "  BOTHO_BINARY_URL unset; reusing existing $BIN_PATH (idempotent re-run)"
else
    # Latest-release fallback: resolve the newest GitHub release and consume
    # its linux-aarch64 tarball, pinning the published `botho` digest unless
    # the operator already supplied one.
    log "  BOTHO_BINARY_URL unset and no $BIN_PATH; resolving latest GitHub release of $BOTHO_REPO"
    LATEST_TAG="$(curl -fsSL --retry 3 --retry-delay 2 \
        "https://api.github.com/repos/${BOTHO_REPO}/releases/latest" 2>/dev/null \
        | grep -m1 '"tag_name"' | cut -d'"' -f4 || true)"
    [[ -n "$LATEST_TAG" ]] \
        || fail "could not resolve the latest release tag of $BOTHO_REPO from the GitHub API. Pass BOTHO_BINARY_URL explicitly (release tarball or bare-binary mirror URL). See infra/baas/README.md."
    RELEASE_BASE="https://github.com/${BOTHO_REPO}/releases/download/${LATEST_TAG}"
    log "  latest release: $LATEST_TAG"
    if [[ -z "$BOTHO_BINARY_SHA256" ]]; then
        # Pin the digest of the extracted `botho` binary — the `botho` line of
        # checksums-linux-aarch64.txt. (Do NOT use SHA256SUMS.txt: it is a
        # platform-unlabelled concatenation with multiple `botho` lines.)
        BOTHO_BINARY_SHA256="$(curl -fsSL --retry 3 --retry-delay 2 \
            "${RELEASE_BASE}/checksums-linux-aarch64.txt" 2>/dev/null \
            | awk '$2 == "botho" {print $1; exit}' || true)"
        [[ -n "$BOTHO_BINARY_SHA256" ]] \
            || fail "could not fetch the 'botho' digest from ${RELEASE_BASE}/checksums-linux-aarch64.txt. Pass BOTHO_BINARY_SHA256 (or an explicit BOTHO_BINARY_URL) instead."
        log "  pinned sha256 from checksums-linux-aarch64.txt: $BOTHO_BINARY_SHA256"
    fi
    install_binary "${RELEASE_BASE}/botho-${LATEST_TAG}-linux-aarch64.tar.gz"
fi
# The binary has no `--version` flag; the version is reported via RPC
# (nodeVersion) once the node is up. Record the architecture here; resolve the
# version in Step 6 after the service starts.
if file "$BIN_PATH" | grep -qi "aarch64"; then BIN_ARCH="aarch64"; else BIN_ARCH="unknown-arch"; fi
log "  installed botho ($BIN_ARCH)"

# ===========================================================================
# Step 3: Generate per-node identity + write config.toml
# ===========================================================================
# Notes on "node_key":
#   The botho binary persists its libp2p peer identity automatically: on first
#   start it creates ~/.botho/<network>/node_key and reloads it on every
#   subsequent boot (log: "Loaded persistent node identity from ... node_key").
#   So the node's peer id is stable across reboots with no action needed here.
#   The *wallet mnemonic* below is the node's economic identity (it owns the
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
elif [[ -n "$NODE_WALLET_MNEMONIC" ]]; then
    log "  using bring-your-own NODE_WALLET_MNEMONIC"
    MNEMONIC="$NODE_WALLET_MNEMONIC"
else
    log "  generating fresh per-node wallet mnemonic"
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

# Bootstrap peers (Fix 2, issue #807).
#
# IMPORTANT: the node's libp2p transport does NOT support bare `/dns4/.../tcp`
# multiaddrs in the explicit `bootstrap_peers` list (it returns
# MultiaddrNotSupported and never connects). Worse, the node's OWN DNS-seed
# parser (botho/src/network/dns_seeds.rs `parse_seed_record`) builds exactly
# that `/dns4/host/tcp/port/p2p/id` form for hostname-form TXT seeds — so even
# a binary WITH config-driven DNS-seed discovery hits the same rejection for
# `seeds.testnet.botho.io`'s `PEER_ID@host:port` records. Empty bootstrap_peers
# therefore idles at peerCount 0 on the released binary.
#
# Durable fix: resolve the seed TXT records IN THIS SCRIPT into the
# transport-accepted `/ip4/<ip>/tcp/<port>/p2p/<peer_id>` form and write them
# into bootstrap_peers. `[network.dns_seeds] enabled = true` is kept regardless
# (additive; helps binaries that gain IP-form discovery later).
#
# Precedence:
#   * explicit BOOTSTRAP_PEERS override -> use it verbatim (must be resolved
#     /ip4/.../p2p/... multiaddrs; bare /dns4 forms will NOT connect).
#   * else resolve $SEED_DOMAIN TXT records -> /ip4/.../p2p/... entries.
#   * else (resolution empty/failed) -> fall back to empty bootstrap_peers with
#     a LOUD log, relying on dns_seeds discovery for binaries that support it.
if [[ -n "$BOOTSTRAP_PEERS" ]]; then
    PEERS_TOML="$(echo "$BOOTSTRAP_PEERS" | tr ',' '\n' | sed -E 's/^ *| *$//g; s/^/    "/; s/$/",/')"
    log "  bootstrap_peers: using explicit BOOTSTRAP_PEERS override"
else
    log "  resolving DNS-seed TXT records from $SEED_DOMAIN into bootstrap_peers"
    PEERS_TOML="$(resolve_seed_peers "$SEED_DOMAIN" || true)"
    if [[ -n "$PEERS_TOML" ]]; then
        log "  resolved $(printf '%s\n' "$PEERS_TOML" | grep -c '/ip4/') seed peer(s) into bootstrap_peers"
        printf '%s\n' "$PEERS_TOML" | sed 's/^/    -> /'
    else
        log "  WARN: no TXT seeds resolved for $SEED_DOMAIN; falling back to empty bootstrap_peers + DNS-seed discovery only (peerCount may stay 0 if the running binary does not support IP-form DNS discovery)"
    fi
fi

if [[ ! -f "$CONFIG_FILE" ]]; then
    log "  writing $CONFIG_FILE"
    cat > "$CONFIG_FILE" <<EOF
# Botho BaaS node config (generated by node-bootstrap.sh on $(ts))
# Testnet mining node. Contains the node wallet mnemonic -> chmod 600.
network_type = "testnet"

[wallet]
mnemonic = "${MNEMONIC}"

[network]
gossip_port = ${GOSSIP_PORT}
rpc_port = ${RPC_PORT}
metrics_port = 19090

# Allow the node's own HTTPS host + local nginx proxy to call RPC.
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
Description=Botho BaaS Node (Testnet Mining Node)
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
# Step 5: nginx + TLS + /rpc reverse proxy for the node hostname
# ===========================================================================
if [[ -z "$NODE_HOSTNAME" ]]; then
    log "Step 5: NODE_HOSTNAME unset -> skipping public nginx/TLS (node still serves RPC on localhost:${RPC_PORT})"
else
    log "Step 5: configuring nginx (+TLS mode=$TLS_MODE) for $NODE_HOSTNAME"
    install -d -m 0755 /var/www/certbot
    # NGINX_SITE was seeded from NODE_HOSTNAME at the top; keep it in sync in
    # case NODE_HOSTNAME was derived from NODE_ID after that point.
    NGINX_SITE="/etc/nginx/sites-available/${NODE_HOSTNAME}"

    # Install a stable copy of this script so the TLS-retry systemd unit has a
    # durable path to re-invoke (cloud-init's user-data path is transient).
    install -d -m 0755 "$(dirname "$SELF_INSTALL_PATH")"
    install -m 0755 "$SCRIPT_SELF" "$SELF_INSTALL_PATH"

    # Remove the default site so server_name matching is unambiguous.
    rm -f /etc/nginx/sites-enabled/default

    case "$TLS_MODE" in
        skip)
            write_http_only_site
            ln -sf "$NGINX_SITE" "/etc/nginx/sites-enabled/${NODE_HOSTNAME}"
            nginx -t && systemctl reload nginx
            log "  nginx HTTP-only (/rpc) ready (TLS skipped)"
            ;;
        standalone)
            if ! have_cert; then
                systemctl stop nginx || true
                certbot certonly --standalone -n --agree-tos -m "$CERTBOT_EMAIL" \
                    -d "$NODE_HOSTNAME" || log "  WARN: certbot --standalone failed (DNS not pointed yet?)"
                systemctl start nginx || true
            fi
            if have_cert; then write_tls_site; else write_http_only_site; fi
            ln -sf "$NGINX_SITE" "/etc/nginx/sites-enabled/${NODE_HOSTNAME}"
            nginx -t && systemctl restart nginx
            if have_cert; then
                log "  nginx ready (HTTPS)"
            else
                log "  nginx ready (HTTP-only); installing TLS retry timer"
                install_tls_retry_timer
            fi
            ;;
        webroot|*)
            # Bring up HTTP first so the ACME webroot challenge can be served,
            # then try once inline. If DNS is not yet pointed here, a systemd
            # timer retries certbot until DNS resolves (Fix 1, issue #807) and
            # self-disables once a cert is issued — no manual re-run needed.
            write_http_only_site
            ln -sf "$NGINX_SITE" "/etc/nginx/sites-enabled/${NODE_HOSTNAME}"
            nginx -t && systemctl reload nginx
            if try_issue_tls_cert; then
                log "  nginx HTTPS /rpc ready for https://${NODE_HOSTNAME}/rpc"
            else
                log "  nginx HTTP-only /rpc ready (no cert yet); installing TLS retry timer to acquire it once DNS resolves"
                install_tls_retry_timer
            fi
            ;;
    esac
fi

# ===========================================================================
# Step 6: Emit node info + install a `node-status` read-back helper
# ===========================================================================
log "Step 6: writing node-info + installing node-status helper"

cat > /usr/local/bin/node-status <<EOF
#!/usr/bin/env bash
# Read back this node's node status + RPC URL.
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
chmod +x /usr/local/bin/node-status

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

if [[ -n "$NODE_HOSTNAME" ]]; then
    RPC_URL="https://${NODE_HOSTNAME}/rpc  (or http://${NODE_HOSTNAME}/rpc until TLS issued)"
else
    RPC_URL="http://localhost:${RPC_PORT}  (no public hostname configured)"
fi

cat > "${RUN_HOME}/node-info.txt" <<EOF
# Botho node provisioned by node-bootstrap.sh on $(ts)
network        = ${NETWORK}
node_id         = ${NODE_ID:-<none>}
node_hostname   = ${NODE_HOSTNAME:-<none>}
region         = ${REGION:-<unset>}
tier           = ${TIER}
public_ip      = ${PUBLIC_IP}
binary_version = ${BIN_VERSION}
rpc_url        = ${RPC_URL}
local_rpc      = http://localhost:${RPC_PORT}
config         = ${CONFIG_FILE}   (mnemonic inside, chmod 600)
service        = systemctl status botho
logs           = journalctl -u botho -f
status         = sudo node-status
EOF
chown "$RUN_USER:$RUN_USER" "${RUN_HOME}/node-info.txt"

# Give the node a moment, then log a first status (best effort).
sleep 5
log "Initial node status:"
/usr/local/bin/node-status 2>&1 | sed 's/^/    /' || true

log "=== Botho node bootstrap complete ==="
log "Read back any time: sudo node-status   |   cat ${RUN_HOME}/node-info.txt"

# ---------------------------------------------------------------------------
# BINARY SOURCE NOTE (for #458)
# ---------------------------------------------------------------------------
# This bootstrap DOWNLOADS the prebuilt arm64 binary (it never builds from
# source on the box — t4g release builds are slow and RandomX-linked crates can
# OOM). Since v0.3.0 (2026-07-05) the canonical source is the GitHub RELEASE
# ASSET published by .github/workflows/release.yml:
#
#     botho-vX.Y.Z-linux-aarch64.tar.gz   (gzip tarball; top-level members
#                                          `botho`, `botho-wallet`,
#                                          `botho-exchange-scanner`, mode 0644
#                                          inside the archive — this script
#                                          extracts `botho` and installs it
#                                          0755)
#     checksums-linux-aarch64.txt         (sha256 of each EXTRACTED binary,
#                                          one per line — the tarball's own
#                                          digest is published nowhere)
#
# Verification: BOTHO_BINARY_SHA256 must be the `botho` line of
# checksums-linux-aarch64.txt (i.e. the digest of the extracted binary). Do
# NOT take digests from SHA256SUMS.txt — it concatenates every platform's
# checksums with no platform labels, so `botho` appears multiple times with
# conflicting digests.
#
# Resolution order implemented in Step 2:
#   1. BOTHO_BINARY_URL set  -> download it. Accepts the release tarball OR a
#      bare aarch64 binary (legacy S3/R2 mirror path, kept for backward
#      compatibility; in that mode the digest is of the downloaded file).
#   2. else, $BIN_PATH exists -> reuse it (idempotent re-run; no network).
#   3. else -> resolve the latest GitHub release of $BOTHO_REPO via the GitHub
#      API and consume its linux-aarch64 tarball, auto-pinning the `botho`
#      digest from checksums-linux-aarch64.txt when BOTHO_BINARY_SHA256 is
#      unset. If the API is unreachable, the script fails with instructions to
#      pass BOTHO_BINARY_URL explicitly.
#
# The provisioner (#458 P6.2) may therefore omit both variables entirely
# (latest release), or pin an exact build by passing the release-asset URL +
# the published `botho` digest. (Historical: pre-v0.3.0 releases shipped no
# downloadable assets, which required an interim "copy from a live seed"
# stand-in — that guidance is obsolete.) See infra/baas/README.md.
