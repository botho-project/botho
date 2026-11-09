#!/bin/bash
# One #1987 m2 campaign run, idempotent (skips a finished output).
#
# Usage: redistribution_shapes_run_one.sh "<outfile>\t<cluster-tax-sim args...>"
# (one line of redistribution_shapes_jobs.py output). The binary defaults to
# target/release/cluster-tax-sim; override with CLUSTER_TAX_SIM.
#
# Full campaign (384 runs, about 20.5 core-hours: median 134 s, max 646 s per
# run; 56 min wall on 22 workers of a shared 28-core Apple M-series):
#   python3 scripts/research/redistribution_shapes_jobs.py /tmp/shapes-runs > /tmp/jobs.tsv
#   tr '\n' '\0' < /tmp/jobs.tsv | xargs -0 -n1 -P 22 scripts/research/redistribution_shapes_run_one.sh
#   python3 scripts/research/redistribution_shapes_tables.py /tmp/shapes-runs
B="${CLUSTER_TAX_SIM:-target/release/cluster-tax-sim}"
IFS=$'\t' read -r -a parts <<< "$1"
out="${parts[0]}"
args=("${parts[@]:1}")
mkdir -p "$(dirname "$out")"
if [ -s "$out" ] && [ "$(wc -l < "$out")" -ge 2 ]; then
  exit 0
fi
tmp="$out.tmp"
if nice -n 10 "$B" "${args[@]}" > "$tmp" 2> "$out.err"; then
  mv "$tmp" "$out"
  rm -f "$out.err"
else
  echo "FAILED: ${args[*]}" >> "$out.err"
fi
