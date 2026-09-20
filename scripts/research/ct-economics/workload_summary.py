#!/usr/bin/env python3
"""Compact exact-input provenance and paired finite-workload aggregates."""
import hashlib
import json
import platform
from pathlib import Path
import subprocess

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
raw = (HERE / 'workload-results.json').read_bytes()
data = json.loads(raw)
config_raw = (HERE / 'workload-config.json').read_bytes()
config = json.loads(config_raw)
assert data['config'] == config
rows = data['rows']
expected = {(s, c, t) for s in config['seeds'] for c in config['honest_cadences'] for t in config['strategies']}
assert len(rows) == len(expected) == 16
assert {(r['seed'], r['honest_cadence'], r['strategy']) for r in rows} == expected
initial = config['initial_bth_per_owner'] * 10**12
for row in rows:
    assert row['honest_attempts'] == (config['blocks'] - 1) // row['honest_cadence'] + 1
    for who in ('honest', 'attacker'):
        failed = sum(n for reason, n in row['failures'].items() if reason.startswith(who + ':'))
        assert row[who + '_attempts'] == row[who + '_success'] + failed
    assert sum(row['honest_fee_histogram'].values()) == row['honest_success']
    assert sum(int(fee) * n for fee, n in row['honest_fee_histogram'].items()) == int(row['honest_fees'])
    assert int(row['honest_fees']) + int(row['attacker_fees']) == int(row['awarded']) + int(row['burn']) + int(row['reserve'])
    assert int(row['attacker_accounted_value']) + int(row['honest_accounted_value']) + int(row['burn']) + int(row['reserve']) == initial * (config['honest_owners'] + 1)
    assert int(row['attacker_accounted_value']) - initial == int(row['attacker_capture']) - int(row['attacker_fees'])
    for who in ('honest', 'attacker'):
        assert int(row[who + '_accounted_value']) == int(row[who + '_spendable']) + int(row[who + '_locked_payouts'])
    assert int(row['attacker_spendable']) == initial - int(row['attacker_fees'])
    assert int(row['attacker_locked_payouts']) == int(row['attacker_capture'])
    assert row['public_outputs'] <= config['max_public_outputs'] < config['max_eligible_candidates']
    assert row['max_spent_public_eligible'] > 0 and row['max_payout_eligible'] > 0
    baseline = next(r for r in rows if r['seed'] == row['seed'] and r['honest_cadence'] == row['honest_cadence'] and r['strategy'] == 'hold_one')
    row['accounted_delta_idle'] = str(int(row['attacker_accounted_value']) - initial)
    row['accounted_delta_hold_one'] = str(int(row['attacker_accounted_value']) - int(baseline['attacker_accounted_value']))
paths = ['Cargo.lock', 'botho/tests/ct_economics_workload.rs', 'scripts/research/ct-economics/workload-config.json', 'scripts/research/ct-economics/workload_summary.py', 'scripts/research/ct-economics/reference.rs', 'botho/src/ledger/store.rs', 'botho/src/ledger/snapshot.rs', 'botho/src/wallet.rs', 'botho/src/decoy_selection.rs', 'botho/src/consensus/lottery.rs', 'botho/src/block.rs', 'cluster-tax/src/lottery.rs', 'cluster-tax/src/demurrage.rs', 'cluster-tax/src/monetary.rs', '.github/workflows/workspace-build.yml']
result = dict(schema=1, scope='Inactive synthetic fixed-stock funded-payment histories; node gamma membership only, not full wallet construction, observed calibration, equilibrium, or ratification',
    checkout_commit=subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=ROOT, text=True).strip(),
    runtime=dict(platform=platform.platform(), rustc=subprocess.check_output(['rustc', '--version'], text=True).strip()),
    raw_sha256=hashlib.sha256(raw).hexdigest(), config_sha256=hashlib.sha256(config_raw).hexdigest(),
    source_sha256={path: hashlib.sha256((ROOT / path).read_bytes()).hexdigest() for path in paths}, config=config, rows=rows)
# One compact row per line keeps the checked-in aggregate small and diffable.
metadata = dict(result)
metadata.pop('rows')
prefix = json.dumps(metadata, indent=2)[:-2]
text = prefix + ',\n  "rows": [\n' + ',\n'.join('    ' + json.dumps(row, separators=(',', ':')) for row in rows) + '\n  ]\n}\n'
assert json.loads(text) == result
(HERE / 'workload-summary.json').write_text(text)
print(f'PASS: {len(rows)} funded workload rows, conservation/denominators verified, {len(paths)} source hashes')
