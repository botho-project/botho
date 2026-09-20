#!/usr/bin/env python3
"""Summarize actual local outputs; never manufacture missing observations."""
import hashlib
import json
from pathlib import Path
import platform
import subprocess

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
raw = (HERE / 'results.json').read_bytes()
data = json.loads(raw)
rows = []
for row in data['rows']:
    assert row['selected'] + row['failures'] == row['attempts']
    if row['selected'] < 2:
        assert row['largest_repeat_overlap_with_first'] is None
        assert row['max_jaccard_with_first'] is None
    if row['scenario'] == 'high-young-tail' and row['route'] == 'web_rpc_order':
        assert row['outside_age_band'] == 1  # lower-bound violations count too
    for e in row['evaluations']:
        assert e['affordable'] + e['unaffordable'] == e['selected']
        assert e['selected'] + e['selection_failures'] == row['attempts']
    evaluations = [e for e in row['evaluations'] if e['rate'] == 200 and e['bits'] == 2
                   and e['outputs'] == 1 and e['output_factor'] == 1000]
    # Deduplicate equivalent same-factor/background evaluations for real1000.
    by_value = {e['value']: e for e in evaluations}
    rows.append({k: v for k, v in row.items() if k not in ('evaluations', 'membership_counts')} |
                {'selected_fee_evaluations': list(by_value.values())})
assert len(rows) == 48
paths = [
    'botho/tests/ct_economics_simulation.rs', 'botho/src/decoy_selection.rs',
    'botho/src/ledger/store.rs', 'botho-wallet/src/ring_builder.rs',
    'botho-wallet/src/decoy_selection.rs', 'web/packages/wasm-signer/src/send.ts',
    'web/packages/wasm-signer/test/ct-economics-selection.test.ts',
    'botho/src/consensus/lottery.rs', 'botho/src/block.rs', 'botho/src/monetary.rs',
    'cluster-tax/src/monetary.rs', 'cluster-tax/src/demurrage.rs', 'cluster-tax/src/fee_curve.rs', 'cluster-tax/src/lottery.rs',
    'scripts/research/ct-economics/reference.rs', 'scripts/research/ct-economics/scenarios.json',
    'scripts/research/ct-economics/pools.json', 'scripts/research/ct-economics/web-selection.json',
    'scripts/research/ct-economics/summarize.py',
]
# CI checks out a shallow merge commit and may have no origin/main ref. The
# measured files are bound by hashes below; do not invent a base when Git cannot
# establish it, or discard otherwise valid observations for missing history.
base = subprocess.run(['git', 'merge-base', 'HEAD', 'origin/main'], cwd=ROOT,
                      text=True, capture_output=True, check=False)
summary = {
    'scope': data['scope'], 'seeds': data['seeds'], 'draws_per_seed': data['draws_per_seed'],
    'runtime': platform.platform(),
    'rustc': subprocess.check_output(['rustc', '--version'], text=True).strip(),
    'checkout_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
    'base_commit': base.stdout.strip() if base.returncode == 0 else None,
    'base_commit_status': 'resolved' if base.returncode == 0 else 'unavailable in checkout',
    'source_sha256': {p: hashlib.sha256((ROOT / p).read_bytes()).hexdigest() for p in paths},
    'full_results_sha256': hashlib.sha256(raw).hexdigest(),
    'path_c_sha256': hashlib.sha256((HERE / 'path-c.json').read_bytes()).hexdigest(),
    'rows': rows,
}
(HERE / 'summary.json').write_text(json.dumps(summary, indent=2) + '\n')
print(f'48 rows summarized; {len(paths)} source/fixture hashes; full failure denominators checked')
