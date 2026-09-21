#!/usr/bin/env python3
"""Strict importer for ordinary V1 observations; no power or CT1 extrapolation."""
import argparse
import hashlib
import json
import re
from decimal import Decimal
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
UNITS = {"wall": "nanoseconds", "cpu": "microseconds_RUSAGE_SELF",
         "objects": "bincode_object_bytes_not_wire",
         "storage": "file_bytes_and_selected_record_bytes_separate"}
TEST = "resources::accepted_v1_apply_resource_sample"
COMPILE_COMMAND = ["cargo", "test", "--locked", "--release", "-p", "botho", "--test", "tx_lifecycle_integration", "--no-run", "--message-format=json"]
SOURCES = ["Cargo.toml", "Cargo.lock", "rust-toolchain", "botho/Cargo.toml",
           ".github/workflows/accepted-v1-resources.yml",
           "botho/tests/tx_lifecycle_integration.rs",
           "botho/tests/tx_lifecycle_integration/resources.rs",
           "botho/src/ledger/store.rs", "botho/src/ledger/store/writer.rs",
           "botho/src/block.rs", "botho/src/consensus/lottery.rs",
           "transaction/clsag/src/lib.rs", "botho/src/transaction.rs",
           "botho/src/monetary.rs", "botho/src/decoy_selection.rs",
           "botho-wallet/src/keys.rs", "account-keys/src/account_keys.rs",
           "crypto/keys/src/ristretto.rs", "crypto/keys/src/compat.rs",
           "crypto/ring-signature/src/ring_signature/clsag.rs",
           "crypto/ring-signature/src/ring_signature/mod.rs",
           "crypto/ring-signature/src/compat.rs",
           "cluster-tax/src/monetary.rs", "cluster-tax/src/fee_curve.rs",
           "cluster-tax/src/dynamic_fee.rs", "cluster-tax/src/demurrage.rs",
           "cluster-tax/src/lottery.rs", "cluster-tax/src/tag.rs",
           "cluster-tax/src/bridge_import.rs", "cluster-tax/src/age_decay.rs",
           "scripts/research/resource-frontier/accepted_v1.py",
           "scripts/research/resource-frontier/measure_accepted_v1.py"]


def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def integer(value):
    if type(value) is not int or value < 0:
        raise ValueError("expected nonnegative integer")
    return value


def exact_keys(obj, keys):
    if not isinstance(obj, dict) or set(obj) != set(keys):
        raise ValueError("unexpected/missing fields")


def validate_sample(s):
    if (integer(s['schema']), s['kind'], s['status']) != (1, 'ordinary_v1_accepted_block_apply', 'accepted_reopened'):
        raise ValueError('not accepted ordinary V1 evidence')
    if s['units'] != UNITS:
        raise ValueError('incompatible measurement units')
    n = integer(s['payments'])
    if n not in (0, 1, 2) or any(integer(s[k]) != n for k in ('transactions', 'recipient_outputs', 'change_outputs')):
        raise ValueError('invalid accepted denominators')
    if any(integer(s[k]) != v for k, v in {'coinbase_outputs': 1, 'lottery_outputs': 0, 'ring_size': 20, 'inputs_per_transaction': 1}.items()):
        raise ValueError('unexpected fixture shape')
    for phase in ('setup', 'apply'):
        exact_keys(s[phase], ('wall_ns', 'user_cpu_us', 'system_cpu_us'))
        for value in s[phase].values():
            integer(value)
    objects = s['objects']
    if len(objects['transaction_bytes']) != n:
        raise ValueError('transaction byte denominator mismatch')
    sizes = [integer(v) for v in objects['transaction_bytes']]
    if any(v == 0 for v in sizes):
        raise ValueError('zero transaction object size')
    total = sum(sizes)
    if integer(objects['block_bytes']) < total or objects['block_bytes'] == 0:
        raise ValueError('invalid object size')
    storage = s['storage']
    for phase in ('before_files', 'after_files'):
        exact_keys(storage[phase], ('data.mdb', 'lock.mdb'))
        for values in storage[phase].values():
            exact_keys(values, ('logical_bytes', 'allocated_bytes'))
            for value in values.values():
                integer(value)
    for phase in ('before_selected_records', 'after_selected_records'):
        r = storage[phase]
        if r['scope'] != 'selected_blocks_and_utxos_only':
            raise ValueError('storage scope mismatch')
        expected_rows = 2 if phase == 'before_selected_records' else 4 + 2*n
        if integer(r['rows']) != expected_rows:
            raise ValueError('selected record count mismatch')
        integer(r['encoded_key_value_bytes'])
    ids = storage['selected_utxo_ids']
    if not isinstance(ids, list) or any(not isinstance(v, str) or re.fullmatch('[0-9a-f]{72}', v) is None for v in ids):
        raise ValueError('noncanonical outpoint identity')
    if integer(storage['selected_block_height']) != 21 or len(ids) != 3 + 2*n or len(set(ids)) != len(ids):
        raise ValueError('selected record identity mismatch')
    return s


def select_artifact(path):
    messages = [json.loads(line) for line in Path(path).read_text().splitlines()]
    matches = [a for a in messages if a.get('reason') == 'compiler-artifact'
               and a.get('target', {}).get('name') == 'tx_lifecycle_integration']
    if len(matches) != 1:
        raise ValueError('ambiguous/missing Cargo test artifact')
    a = matches[0]
    target, profile = a['target'], a['profile']
    if (target.get('kind') != ['test'] or target.get('crate_types') != ['bin']
            or not str(target.get('src_path', '')).endswith('/botho/tests/tx_lifecycle_integration.rs')
            or profile.get('test') is not True or not isinstance(profile.get('opt_level'), str)
            or any(type(profile.get(k)) is not bool for k in ('debug_assertions', 'overflow_checks'))
            or not isinstance(a.get('executable'), str) or not Path(a['executable']).is_absolute()):
        raise ValueError('wrong Cargo target/executable/profile')
    if not any(m.get('reason') == 'build-finished' and m.get('success') is True for m in messages):
        raise ValueError('Cargo build did not finish successfully')
    return a


def load_evidence(directory, root=ROOT):
    directory, root = Path(directory), Path(root)
    manifest = json.loads((directory / 'manifest.json').read_text())
    if manifest['status'] != 'complete' or manifest['compile_exit'] != 0:
        raise ValueError('incomplete run')
    if manifest['compile_command'] != COMPILE_COMMAND:
        raise ValueError('unexpected compile command')
    if digest(directory / 'cargo.jsonl') != manifest['cargo_json_sha256']:
        raise ValueError('Cargo JSON hash mismatch')
    artifact = select_artifact(directory / 'cargo.jsonl')
    if artifact != manifest['artifact'] or re.fullmatch('[0-9a-f]{64}', manifest['executable_sha256']) is None:
        raise ValueError('Cargo artifact provenance mismatch')
    if set(manifest['sources']) != set(SOURCES):
        raise ValueError('incomplete source inventory')
    for path, expected in manifest['sources'].items():
        if digest(root / path) != expected:
            raise ValueError(f'source mismatch: {path}')
    samples = []
    expected_cases = {f'payments-{n}-sample-{i}' for n in range(3) for i in range(2)}
    if {r['case'] for r in manifest['runs']} != expected_cases or len(manifest['runs']) != 6:
        raise ValueError('incomplete/duplicate matrix')
    for run in manifest['runs']:
        if run['exit'] != 0 or run['log'] != run['case'] + '.log':
            raise ValueError('failed/misidentified sample')
        if run['command'] != [artifact['executable'], TEST, '--exact', '--ignored', '--nocapture']:
            raise ValueError('sample executable/command mismatch')
        path = directory / run['log']
        if digest(path) != run['sha256']:
            raise ValueError('raw log mismatch')
        records = [line.split('V1_RESOURCE_SAMPLE ', 1)[1] for line in path.read_text().splitlines() if line.startswith('V1_RESOURCE_SAMPLE ')]
        if len(records) != 1:
            raise ValueError('missing/duplicate raw sample')
        s = validate_sample(json.loads(records[0]))
        if run['case'].split('-')[1] != str(s['payments']):
            raise ValueError('wrong case contents')
        samples.append(s)
    return manifest, samples


def observed_inputs(samples):
    rows = []
    for s in samples:
        n = s['payments']
        cpu = s['apply']['user_cpu_us'] + s['apply']['system_cpu_us']
        rows.append({'payments': n, 'apply_cpu_us': cpu, 'apply_wall_ns': s['apply']['wall_ns'],
                     'cpu_us_per_payment': str(Decimal(cpu) / Decimal(n)) if n else None,
                     'objects': s['objects'], 'storage': s['storage']})
    return {'status': 'partially_measured_ordinary_v1_not_network_cost', 'observations': rows,
            'electricity_kwh': None, 'full_wire_bytes': None, 'ct1_cpu_seconds': None,
            'fee_recommendation_bth': None,
            'limits': ['CPU is not energy; wall is not CPU', 'Selected records are not the full database',
                       'No throughput or marginal-cost guarantee', 'Numeric swaps cannot be detected without trusted collector provenance']}


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('directory', type=Path)
    args = parser.parse_args()
    _, samples = load_evidence(args.directory)
    print(json.dumps(observed_inputs(samples), indent=2))
