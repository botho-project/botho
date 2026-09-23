#!/usr/bin/env python3
"""Independent exact owner/receipt accounting for the schema2 candidate model."""
import argparse
import hashlib
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[3]
HERE = Path(__file__).resolve().parent
SOURCES = [
    'Cargo.toml', 'Cargo.lock', 'rust-toolchain', 'botho/Cargo.toml',
    'botho/tests/common/funded_model.rs', 'botho/tests/common/reinvestment_model.rs',
    'botho/tests/ct_economics_reinvestment.rs',
    'scripts/research/ct-economics/reference.rs',
    'scripts/research/ct-reinvestment/config.json', 'scripts/research/ct-reinvestment/check.py',
    'scripts/research/ct-reinvestment/collect.py', 'scripts/research/ct-reinvestment/test_check.py',
    'botho/src/decoy_selection.rs', 'botho/src/ledger/store.rs',
    'botho/src/ledger/store/validation.rs', 'botho/src/ledger/store/writer.rs',
    'botho/src/consensus/lottery.rs', 'botho/src/block.rs', 'botho/src/monetary.rs',
    'cluster-tax/src/lottery.rs', 'cluster-tax/src/demurrage.rs', 'cluster-tax/src/monetary.rs',
]
TEST_TARGET = 'ct_economics_reinvestment'
COMMAND = ['cargo', 'test', '--locked', '--release', '-p', 'botho', '--test',
           TEST_TARGET, '--no-run', '--message-format=json']
MODES = ['legacy_locked', 'candidate_locked', 'candidate_age720', 'candidate_age10001']
INITIAL = 32_000_000_000_000


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def integer(value):
    assert type(value) is int and 0 <= value < 2**128
    return value


def amount(value):
    assert type(value) is str and re.fullmatch(r'0|[1-9][0-9]*', value)
    return integer(int(value))


def identity(value):
    assert type(value) is str and re.fullmatch('[0-9a-f]{72}', value)
    return value


def counts(mapping):
    assert type(mapping) is dict
    return sum(integer(v) for v in mapping.values())


def validate_row(row):
    mode = row['mode']
    assert mode in MODES and row['seed'] in [1306, 902]
    owners = row['owners']
    assert len(owners) == 101 and [integer(o['owner']) for o in owners] == list(range(101))
    parsed = []
    fields = ['ordinary', 'award_unspent', 'award_consumed', 'award_immature', 'award_deferred',
              'award_ready', 'capture', 'fees', 'payments_sent', 'payments_received']
    for o in owners:
        a = {f: amount(o[f]) for f in fields}
        assert a['capture'] == a['award_unspent'] + a['award_consumed']
        assert a['award_unspent'] == a['award_immature'] + a['award_deferred'] + a['award_ready']
        assert (a['ordinary'] + a['award_unspent'] + a['fees'] + a['payments_sent']
                == INITIAL + a['payments_received'] + a['capture'])
        assert integer(o['attempts']) == integer(o['success']) + counts(o['failures'])
        if mode.endswith('locked'):
            assert o['attempts'] == o['success'] == a['award_consumed'] == a['award_ready'] == 0
        else:
            assert o['attempts'] == 113
        parsed.append(a)
    total = lambda key: sum(o[key] for o in parsed)
    assert total('payments_sent') == total('payments_received')
    assert amount(row['fees']) == total('fees')
    assert amount(row['capture']) == total('capture')
    assert amount(row['fees']) == amount(row['capture']) + amount(row['burn']) + amount(row['reserve'])
    assert total('ordinary') + total('award_unspent') + amount(row['burn']) + amount(row['reserve']) == INITIAL * 101
    assert integer(row['payment_attempts']) == 113
    assert row['payment_attempts'] == integer(row['payment_success']) + counts(row['payment_failures'])
    assert integer(row['consolidation_opportunities']) == 113 * 101
    assert integer(row['consolidation_attempts']) == sum(o['attempts'] for o in owners)
    assert integer(row['consolidation_success']) == sum(o['success'] for o in owners)
    assert len(row['receipts']) == row['consolidation_success'] == integer(row['recycled_outputs'])
    consumed, fees, successes = [0]*101, [0]*101, [0]*101
    spent, outputs = set(), set()
    for receipt in row['receipts']:
        owner = integer(receipt['owner']); assert owner < 101
        height = integer(receipt['height']); assert 20000 <= height <= 31231 and height % 100 == 0
        ids = [identity(v) for v in receipt['inputs']]
        assert 1 <= len(ids) <= 16 and len(set(ids)) == len(ids) and not spent.intersection(ids)
        spent.update(ids)
        output = identity(receipt['output']); assert output not in outputs and output not in spent
        outputs.add(output)
        assert amount(receipt['fee']) == 250_000_000_000 * len(ids)  # UNIT * max(inputs, one output), zero background charge
        assert amount(receipt['consumed']) == amount(receipt['fee']) + amount(receipt['value'])
        assert amount(receipt['value']) >= 1_000_000
        consumed[owner] += amount(receipt['consumed']); fees[owner] += amount(receipt['fee']); successes[owner] += 1
    for i, o in enumerate(parsed):
        assert consumed[i] == o['award_consumed'] and fees[i] <= o['fees']
        assert successes[i] == owners[i]['success']
    assert amount(row['fees']) == sum(fees) + row['payment_success'] * 500_000_000_000
    assert integer(row['public_outputs']) <= 5000
    assert integer(row['max_eligible']) <= 10000
    return row


def validate_data(data):
    config = json.loads((HERE / 'config.json').read_text())
    assert integer(data['schema']) == 2 and data['config'] == config
    rows = data['rows']
    assert len(rows) == 8 and {(r['seed'], r['mode']) for r in rows} == {(s, m) for s in [1306, 902] for m in MODES}
    for row in rows:
        validate_row(row)
    return rows


def select_artifact(path):
    messages = []
    for line in Path(path).read_text().splitlines():
        try: messages.append(json.loads(line))
        except json.JSONDecodeError: pass
    selected = [m for m in messages if m.get('reason') == 'compiler-artifact'
                and m['target']['name'] == TEST_TARGET and m.get('executable')]
    assert len(selected) == 1
    a = selected[0]
    assert a['target']['kind'] == ['test'] and a['profile']['test'] is True
    assert a['profile']['opt_level'] == '3'
    assert a['profile']['debug_assertions'] is True and a['profile']['overflow_checks'] is True
    assert any(m.get('reason') == 'build-finished' and m.get('success') is True for m in messages)
    return a


def load(directory):
    directory = Path(directory)
    m = json.loads((directory / 'manifest.json').read_text())
    assert integer(m['schema']) == 2 and m['status'] == 'complete'
    assert m['compile_command'] == COMMAND and m['compile_exit'] == m['matrix_exit'] == 0
    assert set(m['sources']) == set(SOURCES)
    for file, expected in m['sources'].items(): assert digest(ROOT / file) == expected, file
    assert set(m['raw_hashes']) == {'cargo.jsonl', 'compile.log', 'matrix.log', 'matrix-stderr.log', 'raw.json'}
    assert m['compile_bound_seconds'] == 900 and m['matrix_bound_seconds'] == 180
    assert m['histories'] == 8 and m['deterministic_replays'] == 1
    assert '3 passed; 0 failed; 0 ignored' in (directory / 'matrix.log').read_text()
    for file, expected in m['raw_hashes'].items(): assert digest(directory / file) == expected, file
    a = select_artifact(directory / 'cargo.jsonl'); assert a == m['artifact']
    assert m['matrix_command'] == [a['executable'], 'reinvestment::', '--nocapture']
    assert re.fullmatch('[0-9a-f]{64}', m['executable_sha256'])
    return validate_data(json.loads((directory / 'raw.json').read_text()))


if __name__ == '__main__':
    p = argparse.ArgumentParser(description=__doc__); p.add_argument('directory'); args = p.parse_args()
    print(f'PASS {len(load(args.directory))} histories, 808 owner accounts and exact receipts/provenance')
