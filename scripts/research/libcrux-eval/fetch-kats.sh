#!/usr/bin/env bash
# Fetch the NIST ACVP FIPS 203 / FIPS 204 vectors used by tests/kat.rs.
#
# Usage: ./fetch-kats.sh <dest-dir>
#   then: LIBCRUX_EVAL_KAT_DIR=<dest-dir> cargo test --release --test kat -- --nocapture
#
# The files (~16 MB) are not committed. They are pinned to one ACVP-Server
# commit and SHA-256-checked so the memo's results are reproducible.
set -euo pipefail

dest="${1:?usage: fetch-kats.sh <dest-dir>}"
mkdir -p "$dest"

ACVP_COMMIT=975de31eb83d87039ec88934fdc47d8c312b892d # usnistgov/ACVP-Server master, 2026-08-12
base="https://raw.githubusercontent.com/usnistgov/ACVP-Server/${ACVP_COMMIT}/gen-val/json-files"

# name sha256
while read -r name sum; do
  out="$dest/$name.json"
  if [ ! -f "$out" ]; then
    curl -sfL "$base/$name/internalProjection.json" -o "$out"
  fi
  got=$(shasum -a 256 "$out" | cut -d' ' -f1)
  if [ "$got" != "$sum" ]; then
    echo "checksum mismatch for $name: got $got want $sum" >&2
    exit 1
  fi
  echo "ok  $name"
done <<'EOF'
ML-KEM-keyGen-FIPS203 d7a62a2c3476957f56dd8d24f9004ea6776ccfe995ffe71a65bb9506dc9c7b1b
ML-KEM-encapDecap-FIPS203 a556952ce869bb89c3a3196a701dad89647c193a34c86eafb61a9d710d5b810f
ML-DSA-keyGen-FIPS204 e67ee6540d40e11506c3c4e3b1f79fc1cefcd49820db99fc61f87cc8ba463baf
ML-DSA-sigGen-FIPS204 72dcaf5f69853ca267ccd16af9cb40949786aca0fcfbf05d1ebeba132b93af22
ML-DSA-sigVer-FIPS204 47cdd6314c7f746d02421ffcba89d4dbc7bb875ac49e07a029fdfc26fba55437
EOF
