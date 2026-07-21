#!/usr/bin/env bash
#
# Validator Gossip Firewall (port 17100)
#
# Renders + loads a peer-allowlist iptables ruleset for the Botho consensus
# gossip port (17100) from a single editable allowlist (gossip-peers.conf), and
# persists it so it survives a reboot. This replaces the hand-typed `iptables`
# commands that TESTNET_RESET.md section 5 used to document — those were never
# persisted and never updated for the eu/ap regional relays, which silently
# stranded the relays in #1114.
#
# Policy: :17100 is PEER-ALLOWLISTED (not open). Only the peers in
# gossip-peers.conf (the US validators' internal VPC IPs + the eu/ap relays'
# public IPs) plus 127.0.0.1 may gossip; everything else is DROPped. The AWS
# security group shows :17100 open to the world, so iptables is the real gate.
#
# Usage:
#   ./gossip-firewall.sh apply    # render + load the ruleset, persist to disk
#   ./gossip-firewall.sh status   # show live :17100 rules + whether persisted
#   ./gossip-firewall.sh remove   # drop our ruleset (revert :17100 to SG-open)
#
# Options:
#   --dry-run          Print every iptables/apt/netfilter command, change nothing
#   --peers FILE       Allowlist file (default: gossip-peers.conf next to script)
#   --help, -h         Show this help and exit
#
# Examples:
#   ./gossip-firewall.sh --dry-run apply    # preview, no changes, no root needed
#   sudo ./gossip-firewall.sh apply         # load + persist on a validator host
#   ./gossip-firewall.sh status             # is :17100 locked down and durable?
#
# OPERATOR SCOPE: running `apply`/`remove` for real mutates the live kernel
# firewall and requires sudo/root ON the validator host (seed / seed2 /
# faucet.botho.io). Like reset-chain.sh / reset-to-testnet.sh, this is an
# OPERATOR action and is intentionally never invoked by CI. `--dry-run` and
# `--help` need no root and are safe to run anywhere (e.g. on a laptop or in
# CI as a lint/preview) — they never touch iptables.

set -euo pipefail

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
BLUE='\033[0;34m'
YELLOW='\033[1;33m'
NC='\033[0m'

log_info() { echo -e "${GREEN}[INFO]${NC} $1"; }
log_step() { echo -e "${BLUE}[STEP]${NC} $1"; }
log_warn() { echo -e "${YELLOW}[WARN]${NC} $1"; }
log_error() { echo -e "${RED}[ERROR]${NC} $1"; }

usage() { sed -n '2,37p' "$0" | sed 's/^#\s\{0,1\}//'; }

# Configuration
PORT=17100
# iptables comment tag marking rules this script owns. Used to flush our own
# rules idempotently on re-apply without disturbing unrelated INPUT rules.
TAG="botho-gossip-allowlist"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PEERS_FILE="${GOSSIP_PEERS_FILE:-$SCRIPT_DIR/gossip-peers.conf}"
PERSIST_FILE="/etc/iptables/rules.v4"

DRY_RUN=false
COMMAND=""

# Parse arguments (subcommand + flags, in any order)
while [[ $# -gt 0 ]]; do
    case "$1" in
        apply|status|remove)
            COMMAND="$1"
            ;;
        --dry-run)
            DRY_RUN=true
            ;;
        --peers)
            PEERS_FILE="$2"
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        -*)
            log_error "Unknown option: $1"
            usage
            exit 1
            ;;
        *)
            log_error "Unknown command: $1 (expected apply|status|remove)"
            usage
            exit 1
            ;;
    esac
    shift
done

if [[ -z "$COMMAND" ]]; then
    log_error "No command given (expected apply|status|remove)"
    usage
    exit 1
fi

# run: execute a privileged command, or just print it in dry-run mode.
run() {
    if [[ "$DRY_RUN" == "true" ]]; then
        echo "    $*"
    else
        "$@"
    fi
}

# load_peers: read PEERS_FILE into the PEERS array. Strips inline `#` comments
# and blank lines; validates each entry looks like an IPv4 address or CIDR.
PEERS=()
load_peers() {
    if [[ ! -f "$PEERS_FILE" ]]; then
        log_error "Allowlist file not found: $PEERS_FILE"
        exit 1
    fi
    local line ip
    while IFS= read -r line || [[ -n "$line" ]]; do
        # Strip inline comments and surrounding whitespace.
        ip="${line%%#*}"
        ip="${ip//[[:space:]]/}"
        [[ -z "$ip" ]] && continue
        if [[ ! "$ip" =~ ^[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}\.[0-9]{1,3}(/[0-9]{1,2})?$ ]]; then
            log_warn "Skipping malformed allowlist entry: '$ip'"
            continue
        fi
        PEERS+=("$ip")
    done < "$PEERS_FILE"

    if [[ ${#PEERS[@]} -eq 0 ]]; then
        log_warn "Allowlist '$PEERS_FILE' has no usable peer IPs."
        log_warn "On a validator this DROPs every remote peer on :$PORT and breaks consensus."
        log_warn "Fill in the fleet's IPs (see the file's comments) before applying for real."
    fi
}

# flush_our_rules: delete every INPUT rule this script previously added, matched
# by our comment TAG, so `apply` is idempotent and `remove` fully reverts.
flush_our_rules() {
    if [[ "$DRY_RUN" == "true" ]]; then
        echo "    # delete every INPUT rule tagged '$TAG' (repeat until none remain):"
        echo "    while n=\$(sudo iptables -L INPUT --line-numbers -n | awk '/$TAG/{print \$1; exit}'); [ -n \"\$n\" ]; do sudo iptables -D INPUT \"\$n\"; done"
        return
    fi
    local num
    while num="$(sudo iptables -L INPUT --line-numbers -n 2>/dev/null | awk -v t="$TAG" '$0 ~ t {print $1; exit}')"; do
        [[ -z "$num" ]] && break
        sudo iptables -D INPUT "$num"
    done
}

# ensure_persistence: make sure netfilter-persistent is installed (Ubuntu 24.04
# on the seeds) so `netfilter-persistent save` can write /etc/iptables/rules.v4.
ensure_persistence() {
    if [[ "$DRY_RUN" == "true" ]]; then
        run sudo apt-get update
        run sudo apt-get install -y netfilter-persistent iptables-persistent
        return
    fi
    if ! command -v netfilter-persistent >/dev/null 2>&1; then
        log_step "Installing netfilter-persistent / iptables-persistent..."
        run sudo apt-get update
        run sudo DEBIAN_FRONTEND=noninteractive apt-get install -y \
            netfilter-persistent iptables-persistent
    else
        log_info "netfilter-persistent already installed."
    fi
}

# persist: write the current in-kernel ruleset to disk so it survives reboot.
persist() {
    log_step "Persisting ruleset to $PERSIST_FILE (netfilter-persistent save)..."
    run sudo netfilter-persistent save
}

cmd_apply() {
    load_peers
    ensure_persistence

    log_step "Flushing any previously-applied '$TAG' rules (idempotent re-apply)..."
    flush_our_rules

    log_step "Allowlisting :$PORT for ${#PEERS[@]} peer(s) + localhost, DROP the rest..."
    local ip
    for ip in "${PEERS[@]}" 127.0.0.1; do
        run sudo iptables -A INPUT -p tcp --dport "$PORT" -s "$ip" \
            -m comment --comment "$TAG" -j ACCEPT
    done
    # Final catch-all DROP for :17100 (added last so it sits after the ACCEPTs).
    run sudo iptables -A INPUT -p tcp --dport "$PORT" \
        -m comment --comment "$TAG" -j DROP

    persist

    if [[ "$DRY_RUN" == "true" ]]; then
        log_info "Dry run complete. No changes were made."
    else
        log_info "Applied gossip firewall for :$PORT and persisted it."
        log_info "Verify with: $0 status"
    fi
}

cmd_status() {
    log_step "Live in-kernel INPUT rules for :$PORT:"
    if [[ "$DRY_RUN" == "true" ]]; then
        echo "    sudo iptables -L INPUT -n --line-numbers | grep -E '$PORT|$TAG'"
    else
        sudo iptables -L INPUT -n --line-numbers | grep -E "$PORT|$TAG" \
            || log_warn "No :$PORT rules in the running kernel (port is OPEN per the SG)."
    fi

    log_step "Persisted ruleset ($PERSIST_FILE):"
    if [[ "$DRY_RUN" == "true" ]]; then
        echo "    sudo grep -E '$PORT|$TAG' $PERSIST_FILE"
        return
    fi
    if [[ ! -f "$PERSIST_FILE" ]]; then
        log_warn "$PERSIST_FILE does not exist — the live rules are NOT durable."
        log_warn "A reboot will lose them (this is the #1114 failure mode). Run: $0 apply"
        return
    fi
    if sudo grep -qE "$TAG" "$PERSIST_FILE"; then
        log_info "Our '$TAG' rules are present in $PERSIST_FILE (durable across reboot)."
    else
        log_warn "$PERSIST_FILE exists but has no '$TAG' rules — live rules are NOT persisted."
        log_warn "Run '$0 apply' to persist them."
    fi
}

cmd_remove() {
    log_step "Removing all '$TAG' rules from :$PORT (reverts to SG default = open)..."
    flush_our_rules
    persist
    if [[ "$DRY_RUN" == "true" ]]; then
        log_info "Dry run complete. No changes were made."
    else
        log_info "Removed gossip firewall for :$PORT and persisted the cleared state."
    fi
}

if [[ "$DRY_RUN" == "true" ]]; then
    log_warn "DRY RUN: printing commands only; iptables is not touched."
fi

case "$COMMAND" in
    apply)  cmd_apply ;;
    status) cmd_status ;;
    remove) cmd_remove ;;
esac
