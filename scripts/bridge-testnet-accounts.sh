#!/usr/bin/env bash
#
# bridge-testnet-accounts.sh — Phase-B account provisioning for the wBTH bridge
# ============================================================================
#
# Generates the TESTNET keypairs the live wBTH deploy needs, stores the private
# keys in a GITIGNORED directory, and prints ONLY public addresses + a faucet
# checklist. Re-running is idempotent: existing keys are skipped, never
# overwritten (so you cannot nuke an already-funded account by re-running).
#
#   Ethereum / Sepolia (secp256k1): deployer, lp, safe-owner-1/2/3
#   Solana   / devnet   (ed25519):  solana-deployer, solana-lp
#
# SECURITY MODEL
#   - Private keys are written to  .secrets/bridge-testnet/  (0600), which MUST
#     be covered by .gitignore. The script REFUSES to run if the target dir is
#     not git-ignored (guards against committing key material).
#   - Private keys are NEVER printed to stdout and NEVER committed.
#   - TESTNET ONLY. The beta testnet is disposable; do not reuse these keys for
#     mainnet or any account holding real value.
#
# TOOLS (auto-detected; graceful fallback)
#   ETH keygen : `cast wallet new` (foundry) if present, else `openssl` +
#                an embedded pure-Python keccak-256 (EIP-55 address derivation).
#   SOL keygen : `solana-keygen new` if present, else `openssl` (ed25519) +
#                embedded base58 pubkey encoding.
#   Always needs: bash, python3, openssl.
#   Install foundry : https://getfoundry.sh   (curl -L https://foundry.paradigm.sh | bash && foundryup)
#   Install solana  : https://docs.solana.com/cli/install-solana-cli-tools
#
# USAGE
#   scripts/bridge-testnet-accounts.sh            # generate + print
#   SECRETS_DIR=/custom/path scripts/bridge-testnet-accounts.sh
#
set -euo pipefail

# --------------------------------------------------------------------------
# Locate repo + secrets dir
# --------------------------------------------------------------------------
REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null || pwd)"
SECRETS_DIR="${SECRETS_DIR:-$REPO_ROOT/.secrets/bridge-testnet}"

ETH_ROLES=(deployer lp safe-owner-1 safe-owner-2 safe-owner-3)
SOL_ROLES=(solana-deployer solana-lp)

# --------------------------------------------------------------------------
# Preconditions
# --------------------------------------------------------------------------
need() { command -v "$1" >/dev/null 2>&1; }

for t in openssl python3; do
  if ! need "$t"; then
    echo "ERROR: required tool '$t' not found on PATH. Install it and retry." >&2
    exit 1
  fi
done

HAVE_CAST=0; need cast && HAVE_CAST=1
HAVE_SOLANA_KEYGEN=0; need solana-keygen && HAVE_SOLANA_KEYGEN=1

mkdir -p "$SECRETS_DIR"
chmod 700 "$SECRETS_DIR" 2>/dev/null || true

# HARD SAFETY GATE: refuse to write secrets into a directory that git would
# track. `git check-ignore` exits 0 only when the path is ignored.
_probe="$SECRETS_DIR/.gitignore-probe"
: > "$_probe"
if ! git -C "$REPO_ROOT" check-ignore -q "$_probe"; then
  rm -f "$_probe"
  cat >&2 <<EOF
ERROR: $SECRETS_DIR is NOT git-ignored.
Refusing to generate private keys into a tracked directory.
Add this line to .gitignore and retry:

  .secrets/

EOF
  exit 1
fi
rm -f "$_probe"

# --------------------------------------------------------------------------
# Embedded crypto helper (keccak-256 for EIP-55 ETH addresses + base58).
# Written to a temp file so we never echo key material through argv when it
# can be avoided. Validated against known test vectors (see script's PR).
# --------------------------------------------------------------------------
KC="$(mktemp -t bridge-kc.XXXXXX.py)"
trap 'rm -f "$KC" "${SOL_PEM:-}"' EXIT
cat > "$KC" <<'PY'
import sys

def keccak256(msg: bytes) -> bytes:
    RC = [
        0x0000000000000001, 0x0000000000008082, 0x800000000000808A, 0x8000000080008000,
        0x000000000000808B, 0x0000000080000001, 0x8000000080008081, 0x8000000000008009,
        0x000000000000008A, 0x0000000000000088, 0x0000000080008009, 0x000000008000000A,
        0x000000008000808B, 0x800000000000008B, 0x8000000000008089, 0x8000000000008003,
        0x8000000000008002, 0x8000000000000080, 0x000000000000800A, 0x800000008000000A,
        0x8000000080008081, 0x8000000000008080, 0x0000000080000001, 0x8000000080008008,
    ]
    r = [
        [0, 36, 3, 41, 18],
        [1, 44, 10, 45, 2],
        [62, 6, 43, 15, 61],
        [28, 55, 25, 21, 56],
        [27, 20, 39, 8, 14],
    ]
    M = 0xFFFFFFFFFFFFFFFF
    def rol(x, n):
        n %= 64
        return ((x << n) | (x >> (64 - n))) & M
    A = [[0] * 5 for _ in range(5)]
    rate = 136
    m = bytearray(msg)
    m.append(0x01)
    while len(m) % rate != 0:
        m.append(0x00)
    m[-1] |= 0x80
    for off in range(0, len(m), rate):
        block = m[off:off + rate]
        for i in range(rate // 8):
            A[i % 5][i // 5] ^= int.from_bytes(block[i * 8:i * 8 + 8], "little")
        for rnd in range(24):
            C = [A[x][0] ^ A[x][1] ^ A[x][2] ^ A[x][3] ^ A[x][4] for x in range(5)]
            D = [C[(x - 1) % 5] ^ rol(C[(x + 1) % 5], 1) for x in range(5)]
            for x in range(5):
                for y in range(5):
                    A[x][y] ^= D[x]
            B = [[0] * 5 for _ in range(5)]
            for x in range(5):
                for y in range(5):
                    B[y][(2 * x + 3 * y) % 5] = rol(A[x][y], r[x][y])
            for x in range(5):
                for y in range(5):
                    A[x][y] = B[x][y] ^ ((~B[(x + 1) % 5][y]) & B[(x + 2) % 5][y]) & M
            A[0][0] ^= RC[rnd]
    out = bytearray()
    for i in range(4):
        out += A[i % 5][i // 5].to_bytes(8, "little")
    return bytes(out[:32])

_B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
def b58encode(b: bytes) -> str:
    n = int.from_bytes(b, "big")
    s = ""
    while n > 0:
        n, rem = divmod(n, 58)
        s = _B58[rem] + s
    pad = 0
    for c in b:
        if c == 0:
            pad += 1
        else:
            break
    return "1" * pad + s

def eip55(addr20: bytes) -> str:
    lower = addr20.hex()
    h = keccak256(lower.encode()).hex()
    out = "0x"
    for i, c in enumerate(lower):
        out += (c.upper() if (c not in "0123456789" and int(h[i], 16) >= 8) else c)
    return out

if __name__ == "__main__":
    cmd = sys.argv[1]
    if cmd == "selftest":
        assert keccak256(b"").hex() == "c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"
        assert keccak256(b"abc").hex() == "4e03657aea45a94fc7d47ba826c8d667c0d1e6e33a64a036ec44f58fa12d6c45"
        print("SELFTEST OK")
    elif cmd == "eth-addr":
        pub = bytes.fromhex(sys.argv[2])       # X||Y (128 hex), no 04 prefix
        print(eip55(keccak256(pub)[-20:]))
    elif cmd == "b58":
        print(b58encode(bytes.fromhex(sys.argv[2])))
    elif cmd == "sol-json":
        # arg2 = seed hex (32B), arg3 = pubkey hex (32B) -> Solana keypair JSON
        seed = bytes.fromhex(sys.argv[2]); pub = bytes.fromhex(sys.argv[3])
        print("[" + ",".join(str(b) for b in (seed + pub)) + "]")
PY

python3 "$KC" selftest >/dev/null

# --------------------------------------------------------------------------
# Result accumulators (role -> address / file)
# --------------------------------------------------------------------------
declare -a SUMMARY_ROLE=() SUMMARY_ADDR=() SUMMARY_FILE=() SUMMARY_STATE=() SUMMARY_KIND=()

record() { # kind role addr file state
  SUMMARY_KIND+=("$1"); SUMMARY_ROLE+=("$2"); SUMMARY_ADDR+=("$3")
  SUMMARY_FILE+=("$4"); SUMMARY_STATE+=("$5")
}

# --------------------------------------------------------------------------
# Ethereum (secp256k1) keygen
# --------------------------------------------------------------------------
gen_eth() {
  local role="$1"
  local keyfile="$SECRETS_DIR/eth-${role}.key"
  local addrfile="$SECRETS_DIR/eth-${role}.addr"

  if [[ -f "$keyfile" ]]; then
    local addr; addr="$(cat "$addrfile" 2>/dev/null || echo "?")"
    record eth "$role" "$addr" "$keyfile" "exists, skipping"
    return
  fi

  local priv addr
  if [[ "$HAVE_CAST" == 1 ]]; then
    # `cast wallet new` prints "Address: 0x.." and "Private key: 0x..".
    # Captured in this subshell only — never echoed to the terminal.
    local out; out="$(cast wallet new 2>/dev/null)"
    addr="$(printf '%s\n' "$out" | awk -F': ' '/Address/{print $2; exit}')"
    priv="$(printf '%s\n' "$out" | awk -F': ' '/Private key/{print $2; exit}')"
    priv="${priv#0x}"
  else
    local pem; pem="$(mktemp -t bridge-eth.XXXXXX.pem)"
    openssl ecparam -genkey -name secp256k1 -noout -out "$pem" 2>/dev/null
    priv="$(openssl ec -in "$pem" -text -noout 2>/dev/null \
              | sed -n '/priv:/,/pub:/p' | grep -vE 'priv|pub' | tr -d ' :\n')"
    priv="$(printf '%064s' "$priv" | tr ' ' 0)"   # left-pad to 32 bytes
    local pub xy
    pub="$(openssl ec -in "$pem" -pubout -outform DER 2>/dev/null | tail -c 65 | xxd -p | tr -d '\n')"
    xy="${pub:2}"                                  # strip 04 uncompressed prefix
    addr="$(python3 "$KC" eth-addr "$xy")"
    rm -f "$pem"
  fi

  ( umask 077; printf '0x%s\n' "$priv" > "$keyfile" )
  chmod 600 "$keyfile"
  printf '%s\n' "$addr" > "$addrfile"
  record eth "$role" "$addr" "$keyfile" "generated"
}

# --------------------------------------------------------------------------
# Solana (ed25519) keygen
# --------------------------------------------------------------------------
gen_sol() {
  local role="$1"                          # e.g. solana-deployer
  local short="${role#solana-}"            # deployer / lp
  local jsonfile="$SECRETS_DIR/solana-${short}.json"
  local pubfile="$SECRETS_DIR/solana-${short}.pub"

  if [[ -f "$jsonfile" ]]; then
    local pub; pub="$(cat "$pubfile" 2>/dev/null || echo "?")"
    record sol "$role" "$pub" "$jsonfile" "exists, skipping"
    return
  fi

  local pub
  if [[ "$HAVE_SOLANA_KEYGEN" == 1 ]]; then
    ( umask 077; solana-keygen new --no-bip39-passphrase --silent --force -o "$jsonfile" >/dev/null 2>&1 )
    pub="$(solana-keygen pubkey "$jsonfile" 2>/dev/null)"
  else
    SOL_PEM="$(mktemp -t bridge-sol.XXXXXX.pem)"
    openssl genpkey -algorithm ED25519 -out "$SOL_PEM" 2>/dev/null
    local seed pubhex
    seed="$(openssl pkey -in "$SOL_PEM" -outform DER 2>/dev/null | tail -c 32 | xxd -p | tr -d '\n')"
    pubhex="$(openssl pkey -in "$SOL_PEM" -pubout -outform DER 2>/dev/null | tail -c 32 | xxd -p | tr -d '\n')"
    ( umask 077; python3 "$KC" sol-json "$seed" "$pubhex" > "$jsonfile" )
    pub="$(python3 "$KC" b58 "$pubhex")"
    rm -f "$SOL_PEM"; SOL_PEM=""
  fi

  chmod 600 "$jsonfile"
  printf '%s\n' "$pub" > "$pubfile"
  record sol "$role" "$pub" "$jsonfile" "generated"
}

# --------------------------------------------------------------------------
# Generate
# --------------------------------------------------------------------------
echo "Bridge testnet account provisioning (Phase B)"
echo "Secrets dir : $SECRETS_DIR  (git-ignored, 0700)"
echo "ETH keygen  : $([[ $HAVE_CAST == 1 ]] && echo 'cast (foundry)' || echo 'openssl + embedded keccak-256')"
echo "SOL keygen  : $([[ $HAVE_SOLANA_KEYGEN == 1 ]] && echo 'solana-keygen' || echo 'openssl (ed25519) + embedded base58')"
echo

for role in "${ETH_ROLES[@]}"; do gen_eth "$role"; done
for role in "${SOL_ROLES[@]}"; do gen_sol "$role"; done

# --------------------------------------------------------------------------
# Summary table (PUBLIC addresses + file paths only — NO private keys)
# --------------------------------------------------------------------------
echo "== Accounts (public addresses only) =="
printf '%-16s %-46s %-12s %s\n' "ROLE" "PUBLIC ADDRESS" "STATE" "KEY FILE"
printf '%-16s %-46s %-12s %s\n' "----" "--------------" "-----" "--------"
for i in "${!SUMMARY_ROLE[@]}"; do
  printf '%-16s %-46s %-12s %s\n' \
    "${SUMMARY_ROLE[$i]}" "${SUMMARY_ADDR[$i]}" "${SUMMARY_STATE[$i]}" "${SUMMARY_FILE[$i]}"
done
echo

# --------------------------------------------------------------------------
# Emit role -> address env config (public only)
# --------------------------------------------------------------------------
ENV_FILE="$SECRETS_DIR/addresses.env"

# Portable role -> address lookup (bash 3.2 has no associative arrays).
addr_of() {
  local want="$1" i
  for i in "${!SUMMARY_ROLE[@]}"; do
    if [[ "${SUMMARY_ROLE[$i]}" == "$want" ]]; then
      printf '%s' "${SUMMARY_ADDR[$i]}"; return 0
    fi
  done
  printf '?'
}

{
  echo "# Bridge testnet role -> PUBLIC address map (generated by scripts/bridge-testnet-accounts.sh)"
  echo "# TESTNET ONLY. Public addresses only — private keys stay in $SECRETS_DIR/*.key|*.json (git-ignored)."
  echo "# The live-deploy harness (#866/#869) consumes these; see docs/bridge/testnet-e2e-runbook.md (Phase B)."
  echo "BRIDGE_SEPOLIA_DEPLOYER=$(addr_of deployer)"
  echo "BRIDGE_SEPOLIA_LP=$(addr_of lp)"
  echo "BRIDGE_SAFE_OWNER_1=$(addr_of safe-owner-1)"
  echo "BRIDGE_SAFE_OWNER_2=$(addr_of safe-owner-2)"
  echo "BRIDGE_SAFE_OWNER_3=$(addr_of safe-owner-3)"
  echo "BRIDGE_SOLANA_DEPLOYER=$(addr_of solana-deployer)"
  echo "BRIDGE_SOLANA_LP=$(addr_of solana-lp)"
} > "$ENV_FILE"

echo "== Env config (written to $ENV_FILE, git-ignored) =="
cat "$ENV_FILE"
echo
cat <<EOF
Deploy-script mapping (see contracts/ethereum/scripts/deploy.ts + hardhat.config.ts):
  - deploy.ts signs with the deployer PRIVATE key via hardhat's PRIVATE_KEY env.
    Wire it up:  export PRIVATE_KEY="\$(cat $SECRETS_DIR/eth-deployer.key)"
    Then verify the signer matches BRIDGE_SEPOLIA_DEPLOYER above.
  - The 3 safe-owner EOAs are the OWNERS of the Gnosis Safe(s). Deploy the Safe(s)
    from these owners (Safe UI/SDK), then set the resulting Safe ADDRESSES as
    WBTH_ADMIN_SAFE / WBTH_MINTER_SAFE / WBTH_PAUSER_SAFE for deploy.ts (ADR 0002).
    (This script provisions the owner EOAs, NOT the Safe contract addresses.)
  - BRIDGE_SEPOLIA_LP is the LP / relayer EOA used by the DeFi round-trip harness.
EOF
echo

# --------------------------------------------------------------------------
# Faucet checklist
# --------------------------------------------------------------------------
cat <<EOF
== Faucet checklist ==

Ethereum / Sepolia (chain id 11155111) — fund these EOAs with test ETH:
  Faucet options (pick per rate limits / holdings):
    - Alchemy Sepolia faucet  : https://sepoliafaucet.com  (0.5 ETH/day; needs an
                                 Alchemy account, some require mainnet balance)
    - Google Cloud Web3 faucet: https://cloud.google.com/application/web3/faucet/ethereum/sepolia
                                 (0.05 ETH/day, no account gate)
    - pk910 PoW faucet        : https://sepolia-faucet.pk910.de  (mine in-browser
                                 to earn; good for larger top-ups, no rate cap)
  Suggested amounts:
    - deployer      ~0.4 Sepolia ETH  (contract deploys)  -> $(addr_of deployer)
    - lp            ~0.2 Sepolia ETH  (LP / relayer gas)   -> $(addr_of lp)
    - safe-owner-1  ~0.05 Sepolia ETH (Safe setup/sign)    -> $(addr_of safe-owner-1)
    - safe-owner-2  ~0.05 Sepolia ETH (Safe setup/sign)    -> $(addr_of safe-owner-2)
    - safe-owner-3  ~0.05 Sepolia ETH (Safe setup/sign)    -> $(addr_of safe-owner-3)

Solana / devnet — airdrop test SOL (2 SOL/request; rate-limited per day/IP):
    solana airdrop 2 $(addr_of solana-deployer) --url devnet
    solana airdrop 2 $(addr_of solana-lp) --url devnet
  Note: the Solana multisig signers (t-of-n mint authority) are provisioned
  separately; solana-deployer is the upgrade/authority key, solana-lp the LP key.

TESTNET ONLY — the beta testnet is disposable. Do NOT fund or reuse these keys
on mainnet.
EOF

echo
echo "Done. Private keys live under $SECRETS_DIR (0600, git-ignored) and were never printed."
